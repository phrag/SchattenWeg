package de.schattenweg.app

import android.content.Intent
import android.graphics.RectF
import android.net.Uri
import android.util.Log
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.snapshots.SnapshotStateMap
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.viewmodel.compose.viewModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.maplibre.android.MapLibre
import org.maplibre.android.camera.CameraPosition
import org.maplibre.android.camera.CameraUpdateFactory
import org.maplibre.android.geometry.LatLng
import org.maplibre.android.maps.MapLibreMap
import org.maplibre.android.maps.MapView
import org.maplibre.android.maps.Style
import org.maplibre.android.style.layers.CircleLayer
import org.maplibre.android.style.layers.FillLayer
import org.maplibre.android.style.layers.LineLayer
import org.maplibre.android.style.layers.Property
import org.maplibre.android.style.layers.PropertyFactory
import org.maplibre.android.style.sources.GeoJsonSource
import uniffi.schattenweg_core.Camera
import uniffi.schattenweg_core.CameraKind
import uniffi.schattenweg_core.LatLon
import uniffi.schattenweg_core.Place
import uniffi.schattenweg_core.PlaceKind
import kotlin.math.cos
import kotlin.math.roundToInt
import kotlin.math.sin
import org.maplibre.android.geometry.LatLng as MlLatLng

/**
 * The four toggleable layer groups and the style-layer ids each owns. Camera
 * dots and coverage are our own overlay layers; labels and buildings are the
 * basemap's, named to match style_template.json. Keep these in sync with the
 * style.
 */
private enum class LayerGroup(val label: String, val ids: List<String>) {
    CAMERAS("Cameras", listOf("sw-camera-dots")),
    COVERAGE("Camera coverage", listOf("sw-camera-coverage-fill")),
    LABELS("Labels", listOf("label-street", "label-transit", "label-place")),
    BUILDINGS(
        "Buildings & landuse",
        listOf("building", "landuse-residential", "landcover", "park"),
    ),
}

private const val CAMERA_SOURCE = "sw-cameras"
private const val COVERAGE_SOURCE = "sw-camera-coverage"

/** Layer id, needed to hit-test taps against the camera dots. */
private const val CAMERA_LAYER = "sw-camera-dots"
private const val ROUTE_SOURCE = "sw-route"
private const val ENDPOINT_SOURCE = "sw-endpoints"

/** Berlin, Alexanderplatz — where the map opens. */
private val BERLIN = LatLng(52.5216, 13.4127)

/** Where the in-app credit points: the project on GitHub. */
private const val PROJECT_URL = "https://github.com/phrag/SchattenWeg"

/** The maintainer's GitHub profile, shown as "by phrag" in the About section. */
private const val MAINTAINER_URL = "https://github.com/phrag"

/**
 * The one screen: a full-bleed offline map with the camera layer, the planned
 * route, and the paranoia slider pinned to the bottom.
 *
 * Tap once to drop a start, twice to plan; a third tap starts over.
 */
@Composable
fun MapScreen(viewModel: RouteViewModel = viewModel()) {
    val context = LocalContext.current
    val state by viewModel.state.collectAsState()
    val level by viewModel.level.collectAsState()
    val cameras by viewModel.cameras.collectAsState()
    val route by viewModel.route.collectAsState()
    val start by viewModel.start.collectAsState()
    val end by viewModel.end.collectAsState()

    val mapAssets by viewModel.mapAssets.collectAsState()
    val searchQuery by viewModel.searchQuery.collectAsState()
    val searchResults by viewModel.searchResults.collectAsState()
    val basemapReady by viewModel.basemapReady.collectAsState()
    val mapView = remember {
        MapLibre.getInstance(context)
        MapView(context)
    }
    // Held so the zoom buttons can drive the camera. The click listener below
    // is registered once, so selection is kept in a MutableState it can write
    // to rather than a value it would capture stale.
    val mapRef = remember { mutableStateOf<MapLibreMap?>(null) }
    val selectedCameraId = remember { mutableStateOf<Long?>(null) }
    val layersOn: SnapshotStateMap<LayerGroup, Boolean> = remember {
        mutableStateMapOf(*LayerGroup.entries.map { it to true }.toTypedArray())
    }
    val panelOpen = remember { mutableStateOf(false) }
    // The avoidance panel starts open (so the honesty notes are seen) but can
    // be slid shut to uncover the map.
    val panelCollapsed = remember { mutableStateOf(false) }

    // MapView is a plain Android view with its own lifecycle contract; forward
    // the host lifecycle to it or the renderer leaks.
    val lifecycleOwner = LocalLifecycleOwner.current
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_CREATE -> mapView.onCreate(null)

                Lifecycle.Event.ON_START -> mapView.onStart()

                Lifecycle.Event.ON_RESUME -> mapView.onResume()

                Lifecycle.Event.ON_PAUSE -> mapView.onPause()

                Lifecycle.Event.ON_STOP -> mapView.onStop()

                // ON_DESTROY is deliberately not handled here: onDispose below
                // owns teardown, so onDestroy is called exactly once.
                else -> Unit
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
            mapView.onDestroy()
        }
    }

    val selectedCamera = cameras.firstOrNull { it.osmId == selectedCameraId.value }

    Box(Modifier.fillMaxSize()) {
        // The style can only be built once provisioning has told us whether
        // offline tiles exist, so it is applied here rather than in factory().
        LaunchedEffect(basemapReady, mapAssets) {
            if (!basemapReady) return@LaunchedEffect
            // Captured into a local: delegated properties do not smart-cast.
            val assets = mapAssets
            val tiles = assets?.pmtiles
            if (tiles == null) {
                Log.w(
                    TAG,
                    "No basemap bundled: rendering cameras and routes " +
                        "on a plain background. Run scripts/build_map_assets.sh.",
                )
            } else {
                Log.i(
                    TAG,
                    "Basemap: ${tiles.absolutePath} (${tiles.length()} bytes), " +
                        "glyphs: ${assets.glyphsDir?.name ?: "none"}",
                )
            }
            val styleJson = MapAssets.styleJson(
                context,
                assets ?: MapAssets.Provisioned(null, null, null),
            )
            mapView.getMapAsync { map ->
                map.setStyle(Style.Builder().fromJson(styleJson)) { style ->
                    style.addSource(GeoJsonSource(COVERAGE_SOURCE, EMPTY_COLLECTION))
                    style.addSource(GeoJsonSource(ROUTE_SOURCE, EMPTY_COLLECTION))
                    style.addSource(GeoJsonSource(CAMERA_SOURCE, EMPTY_COLLECTION))
                    style.addSource(GeoJsonSource(ENDPOINT_SOURCE, EMPTY_COLLECTION))

                    // What the router believes each camera sees. Drawn first
                    // so the dots and the route stay on top of it.
                    style.addLayer(
                        FillLayer("sw-camera-coverage-fill", COVERAGE_SOURCE)
                            .withProperties(
                                PropertyFactory.fillColor("#e0575b"),
                                PropertyFactory.fillOpacity(0.16f),
                            )
                            .also { it.minZoom = 14f },
                    )
                    style.addLayer(
                        LineLayer("sw-route-line", ROUTE_SOURCE).withProperties(
                            PropertyFactory.lineColor("#7fd4a2"),
                            PropertyFactory.lineWidth(5f),
                            PropertyFactory.lineCap("round"),
                            PropertyFactory.lineJoin("round"),
                        ),
                    )
                    style.addLayer(
                        CircleLayer(CAMERA_LAYER, CAMERA_SOURCE).withProperties(
                            PropertyFactory.circleRadius(5f),
                            PropertyFactory.circleColor("#e0575b"),
                            PropertyFactory.circleOpacity(0.85f),
                            PropertyFactory.circleStrokeWidth(1f),
                            PropertyFactory.circleStrokeColor("#2a0f11"),
                        ),
                    )
                    style.addLayer(
                        CircleLayer("sw-endpoint-dots", ENDPOINT_SOURCE).withProperties(
                            PropertyFactory.circleRadius(7f),
                            PropertyFactory.circleColor("#f2f4f8"),
                            PropertyFactory.circleStrokeWidth(2f),
                            PropertyFactory.circleStrokeColor("#10141a"),
                        ),
                    )

                    // The style can finish before factory() has applied the
                    // camera, leaving the target null or at null island. Berlin
                    // is where this map always opens, so ask about it directly
                    // rather than querying the middle of the Atlantic.
                    val centre = map.cameraPosition.target
                        ?.takeIf { it.latitude != 0.0 || it.longitude != 0.0 }
                        ?: BERLIN
                    viewModel.refreshCameras(
                        LatLon(centre.latitude, centre.longitude),
                        map.viewportRadiusM(),
                    )
                }
            }
        }

        // Overlay data is pushed from here rather than from AndroidView's
        // update block. The writes happen inside getMapAsync's callback, and
        // snapshot reads made inside an async callback are not recorded as
        // recomposition dependencies -- so update() would run once with empty
        // data and never again. Naming the values as keys makes the dependency
        // explicit and survives the callback.
        LaunchedEffect(cameras, route, start, end) {
            // Serialising a few thousand features is real work; a whole-city
            // viewport makes it large enough to drop frames on the main thread.
            val cameraJson = withContext(Dispatchers.Default) { camerasGeoJson(cameras) }
            val coverageJson = withContext(Dispatchers.Default) { coverageGeoJson(cameras) }
            val routeJson = routeGeoJson(route?.polyline)
            val endpointJson = pointsGeoJson(listOfNotNull(start, end))
            mapView.getMapAsync { map ->
                val style = map.style
                if (style == null) {
                    Log.w(TAG, "Overlay update skipped: style not ready yet.")
                    return@getMapAsync
                }
                val cameraSource = style.getSourceAs<GeoJsonSource>(CAMERA_SOURCE)
                val coverageSource = style.getSourceAs<GeoJsonSource>(COVERAGE_SOURCE)
                val routeSource = style.getSourceAs<GeoJsonSource>(ROUTE_SOURCE)
                val endpointSource = style.getSourceAs<GeoJsonSource>(ENDPOINT_SOURCE)
                Log.d(
                    TAG,
                    "Overlays: cameras=${cameras.size} " +
                        "route=${route?.polyline?.size ?: 0} " +
                        "endpoints=${listOfNotNull(start, end).size} " +
                        "sources=[${cameraSource != null},${routeSource != null}," +
                        "${endpointSource != null}]",
                )
                cameraSource?.setGeoJson(cameraJson)
                coverageSource?.setGeoJson(coverageJson)
                routeSource?.setGeoJson(routeJson)
                endpointSource?.setGeoJson(endpointJson)
            }
        }

        // Push layer visibility to the style. Keyed on the toggle map and the
        // asset load so it re-applies when the style is rebuilt.
        LaunchedEffect(layersOn.toMap(), basemapReady) {
            mapView.getMapAsync { map ->
                val style = map.style ?: return@getMapAsync
                for (group in LayerGroup.entries) {
                    val visible = layersOn[group] != false
                    for (id in group.ids) {
                        style.getLayer(id)?.setProperties(
                            PropertyFactory.visibility(
                                if (visible) Property.VISIBLE else Property.NONE,
                            ),
                        )
                    }
                }
            }
        }

        AndroidView(
            factory = {
                // Without these, a renderer or tile failure is invisible: the
                // map just stays blank.
                mapView.addOnDidFailLoadingMapListener(
                    MapView.OnDidFailLoadingMapListener { error ->
                        Log.e(TAG, "MapLibre failed to load the map: $error")
                    },
                )
                mapView.addOnDidFinishLoadingStyleListener(
                    MapView.OnDidFinishLoadingStyleListener {
                        Log.i(TAG, "Map style loaded.")
                    },
                )
                mapView.getMapAsync { map ->
                    mapRef.value = map
                    map.cameraPosition = CameraPosition.Builder()
                        .target(BERLIN)
                        .zoom(14.0)
                        .build()

                    map.addOnMapClickListener { point ->
                        // A tap on a camera asks about it; a tap on the map
                        // routes. Hit-test with a finger-sized box, not the
                        // exact pixel.
                        val at = map.projection.toScreenLocation(point)
                        val touch = RectF(at.x - 28f, at.y - 28f, at.x + 28f, at.y + 28f)
                        val hit = map.queryRenderedFeatures(touch, CAMERA_LAYER)
                            .firstOrNull()
                            ?.getNumberProperty("osm_id")
                            ?.toLong()
                        if (hit != null) {
                            Log.d(TAG, "Camera tapped: osm id $hit")
                            selectedCameraId.value = hit
                        } else {
                            Log.d(TAG, "Map tapped at ${point.latitude},${point.longitude}")
                            selectedCameraId.value = null
                            viewModel.onMapTap(LatLon(point.latitude, point.longitude))
                        }
                        true
                    }

                    map.addOnCameraIdleListener {
                        val centre = map.cameraPosition.target ?: return@addOnCameraIdleListener
                        viewModel.refreshCameras(
                            LatLon(centre.latitude, centre.longitude),
                            map.viewportRadiusM(),
                        )
                    }
                }
                mapView
            },
            modifier = Modifier.fillMaxSize(),
        )

        Column(
            Modifier
                .align(Alignment.TopCenter)
                .fillMaxWidth()
                // targetSdk 36 draws edge to edge, so without this it sits
                // under the clock and status icons.
                .statusBarsPadding()
                .padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            SearchBar(
                query = searchQuery,
                results = searchResults,
                onQueryChange = { viewModel.searchQuery.value = it },
                onPick = { place ->
                    mapRef.value?.animateCamera(
                        CameraUpdateFactory.newLatLngZoom(
                            MlLatLng(place.lat, place.lon),
                            15.0,
                        ),
                    )
                    viewModel.clearSearch()
                },
                onUse = { place, asStart ->
                    val point = LatLon(place.lat, place.lon)
                    if (asStart) viewModel.setStart(point) else viewModel.setEnd(point)
                    mapRef.value?.animateCamera(
                        CameraUpdateFactory.newLatLngZoom(
                            MlLatLng(place.lat, place.lon),
                            15.0,
                        ),
                    )
                    viewModel.clearSearch()
                },
            )
            StatusCard(state = state, modifier = Modifier.fillMaxWidth())
        }

        // Layers toggle + zoom controls, clear of both cards.
        Column(
            Modifier
                .align(Alignment.CenterEnd)
                .padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            ZoomButton("\u2261") { panelOpen.value = !panelOpen.value }
            ZoomButton("+") {
                mapRef.value?.animateCamera(CameraUpdateFactory.zoomIn())
            }
            ZoomButton("−") {
                mapRef.value?.animateCamera(CameraUpdateFactory.zoomOut())
            }
        }

        if (panelOpen.value) {
            LayersPanel(
                layersOn = layersOn,
                modifier = Modifier
                    .align(Alignment.CenterEnd)
                    .padding(end = 68.dp),
            )
        }

        Column(
            Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .navigationBarsPadding()
                .padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            selectedCamera?.let { cam ->
                CameraInfoCard(cam) { selectedCameraId.value = null }
            }

            // Avoidance control + the two honesty notes (see CLAUDE.md §5 — these
            // are non-negotiable, so they live in the panel that is open by
            // default). The panel slides shut to uncover the map.
            AvoidancePanel(
                level = level,
                onSelect = {
                    viewModel.level.value = it
                    // A manual pick is honoured as-is: no auto-escalation to a
                    // camera-free route (that is only the default for a freshly
                    // dropped A→B pair).
                    viewModel.plan()
                },
                collapsed = panelCollapsed.value,
                onCollapsedChange = { panelCollapsed.value = it },
            )
            // The map attribution (a licence obligation) and the project links
            // now live in the layers panel's About section. MapLibre's own
            // bottom-left © control keeps OSM attribution on screen regardless.
        }
    }
}

@Composable
private fun SearchBar(
    query: String,
    results: List<Place>,
    onQueryChange: (String) -> Unit,
    onPick: (Place) -> Unit,
    onUse: (Place, Boolean) -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        OutlinedTextField(
            value = query,
            onValueChange = onQueryChange,
            singleLine = true,
            placeholder = { Text("Search a street or place") },
            modifier = Modifier.fillMaxWidth(),
            colors = TextFieldDefaults.colors(
                focusedContainerColor = Color(0xF210141A),
                unfocusedContainerColor = Color(0xF210141A),
                focusedTextColor = Color(0xFFF2F4F8),
                unfocusedTextColor = Color(0xFFF2F4F8),
            ),
        )
        if (results.isNotEmpty()) {
            Card(
                Modifier.fillMaxWidth(),
                colors = CardDefaults.cardColors(containerColor = Color(0xF210141A)),
            ) {
                Column(
                    Modifier
                        .heightIn(max = 260.dp)
                        .verticalScroll(rememberScrollState()),
                ) {
                    for (place in results) {
                        SearchResultRow(place, onPick, onUse)
                    }
                }
            }
        }
    }
}

@Composable
private fun SearchResultRow(
    place: Place,
    onPick: (Place) -> Unit,
    onUse: (Place, Boolean) -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .clickable { onPick(place) }
            .padding(horizontal = 14.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f).padding(end = 8.dp)) {
            Text(
                place.name,
                style = MaterialTheme.typography.bodyMedium,
                color = Color(0xFFF2F4F8),
            )
            Text(
                when (place.kind) {
                    PlaceKind.STREET -> "Street"
                    PlaceKind.LOCALITY -> "District"
                    PlaceKind.STATION -> "Station"
                },
                style = MaterialTheme.typography.labelSmall,
                color = Color(0xFF9AA4B2),
            )
        }
        // Set as start or destination directly from a result.
        Text(
            "A",
            style = MaterialTheme.typography.labelLarge,
            color = Color(0xFF7FD4A2),
            modifier = Modifier
                .clickable { onUse(place, true) }
                .padding(horizontal = 8.dp, vertical = 2.dp),
        )
        Text(
            "B",
            style = MaterialTheme.typography.labelLarge,
            color = Color(0xFF7FD4A2),
            modifier = Modifier
                .clickable { onUse(place, false) }
                .padding(horizontal = 8.dp, vertical = 2.dp),
        )
    }
}

@Composable
private fun LayersPanel(
    layersOn: SnapshotStateMap<LayerGroup, Boolean>,
    modifier: Modifier = Modifier,
) {
    Card(
        modifier,
        colors = CardDefaults.cardColors(containerColor = Color(0xF210141A)),
        shape = RoundedCornerShape(12.dp),
    ) {
        Column(
            Modifier.padding(horizontal = 14.dp, vertical = 10.dp),
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            Text(
                "Layers",
                style = MaterialTheme.typography.labelMedium,
                color = Color(0xFF9AA4B2),
                modifier = Modifier.padding(bottom = 4.dp),
            )
            for (group in LayerGroup.entries) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        group.label,
                        style = MaterialTheme.typography.bodyMedium,
                        color = Color(0xFFF2F4F8),
                        modifier = Modifier.padding(end = 12.dp),
                    )
                    Switch(
                        checked = layersOn[group] != false,
                        onCheckedChange = { layersOn[group] = it },
                        colors = SwitchDefaults.colors(
                            checkedThumbColor = Color(0xFF7FD4A2),
                            checkedTrackColor = Color(0x557FD4A2),
                        ),
                    )
                }
            }
            // Credits live here, out of the way of the map: the map attribution
            // (a licence obligation — OSM data is ODbL, the OpenMapTiles schema
            // CC-BY, both needing a visible credit even offline) alongside the
            // project and maintainer links. MapLibre's own bottom-left © control
            // keeps OSM attribution on screen even when this panel is closed.
            Text(
                "About",
                style = MaterialTheme.typography.labelMedium,
                color = Color(0xFF9AA4B2),
                modifier = Modifier.padding(top = 10.dp),
            )
            Text(
                "© OpenMapTiles © OpenStreetMap contributors",
                style = MaterialTheme.typography.labelSmall,
                color = Color(0xFF6E7A8A),
            )
            LinkText("Schattenweg on GitHub", PROJECT_URL, Modifier.padding(top = 2.dp))
            LinkText("by phrag", MAINTAINER_URL)
        }
    }
}

/**
 * The bottom control: the avoidance picker and the honesty notes, behind a grab
 * handle that slides the whole thing shut to uncover the map. Tap the handle to
 * toggle, or drag it up/down. Open by default so the caveats are seen first.
 */
@Composable
private fun AvoidancePanel(
    level: AvoidanceLevel,
    onSelect: (AvoidanceLevel) -> Unit,
    collapsed: Boolean,
    onCollapsedChange: (Boolean) -> Unit,
) {
    Card(
        colors = CardDefaults.cardColors(containerColor = Color(0xE6161B22)),
    ) {
        Column(Modifier.padding(horizontal = 12.dp, vertical = 8.dp)) {
            Box(
                Modifier
                    .fillMaxWidth()
                    .clickable { onCollapsedChange(!collapsed) }
                    // Key on `collapsed` so the gesture reads the current state,
                    // not the value captured when the modifier was first applied.
                    .pointerInput(collapsed) {
                        detectVerticalDragGestures { _, dragAmount ->
                            if (dragAmount > 4f && !collapsed) onCollapsedChange(true)
                            if (dragAmount < -4f && collapsed) onCollapsedChange(false)
                        }
                    }
                    .padding(vertical = 6.dp),
                contentAlignment = Alignment.Center,
            ) {
                Box(
                    Modifier
                        .size(width = 36.dp, height = 4.dp)
                        .clip(RoundedCornerShape(2.dp))
                        .background(Color(0xFF48505C)),
                )
            }
            AnimatedVisibility(visible = !collapsed) {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(
                        "Camera avoidance",
                        style = MaterialTheme.typography.labelMedium,
                        color = Color(0xFFF2F4F8),
                    )
                    AvoidanceSelector(selected = level, onSelect = onSelect)
                    Text(
                        "Only cameras mapped in OpenStreetMap — real coverage is " +
                            "higher. Avoiding them is not anonymity.",
                        style = MaterialTheme.typography.labelSmall,
                        color = Color(0xFF9AA4B2),
                    )
                }
            }
        }
    }
}

/**
 * Three-way camera-avoidance picker (Low / Medium / High), replacing the old
 * free λ slider. The selected segment is filled; the others read as muted
 * outlines. Mirrors [AvoidanceLevel].
 */
@Composable
private fun AvoidanceSelector(
    selected: AvoidanceLevel,
    onSelect: (AvoidanceLevel) -> Unit,
) {
    Row(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        for (lvl in AvoidanceLevel.entries) {
            val on = lvl == selected
            Card(
                Modifier
                    .weight(1f)
                    .clickable { onSelect(lvl) },
                shape = RoundedCornerShape(10.dp),
                colors = CardDefaults.cardColors(
                    containerColor = if (on) Color(0xFF7FD4A2) else Color(0x22FFFFFF),
                ),
            ) {
                Box(
                    Modifier
                        .fillMaxWidth()
                        .padding(vertical = 10.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        lvl.label,
                        style = MaterialTheme.typography.labelLarge,
                        color = if (on) Color(0xFF10141A) else Color(0xFFC7D0DA),
                    )
                }
            }
        }
    }
}

/**
 * A tappable link that opens [url] in a browser. The app declares no INTERNET
 * permission, but an ACTION_VIEW intent hands the URL to the browser — a
 * different app with its own network access — so this leaks nothing and needs
 * no permission of ours.
 */
@Composable
private fun LinkText(text: String, url: String, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    Text(
        text,
        style = MaterialTheme.typography.labelSmall,
        color = Color(0xFF7FD4A2),
        modifier = modifier.clickable {
            runCatching {
                context.startActivity(
                    Intent(Intent.ACTION_VIEW, Uri.parse(url))
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                )
            }.onFailure { Log.w(TAG, "No app to open $url", it) }
        },
    )
}

@Composable
private fun ZoomButton(label: String, onClick: () -> Unit) {
    Card(
        Modifier
            .size(44.dp)
            .clickable(onClick = onClick),
        colors = CardDefaults.cardColors(containerColor = Color(0xE6161B22)),
    ) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            Text(
                label,
                style = MaterialTheme.typography.titleLarge,
                color = Color(0xFFF2F4F8),
            )
        }
    }
}

/**
 * What this app knows about one camera, and what it is only assuming.
 *
 * The distinction matters: OSM supplies the position, the type and sometimes
 * a bearing. Range and field of view are this project's defaults
 * (`camera::defaults`), chosen conservatively and never tuned against ground
 * truth -- so the card says so rather than presenting them as fact.
 */
@Composable
private fun CameraInfoCard(camera: Camera, onDismiss: () -> Unit) {
    Card(
        Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = Color(0xE6161B22)),
    ) {
        Column(
            Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            val kind = when (camera.kind) {
                CameraKind.FIXED -> "Fixed camera"
                CameraKind.DOME -> "Dome camera"
                CameraKind.PANNING -> "Panning camera"
                CameraKind.UNKNOWN -> "Camera, type not mapped"
            }
            Text(
                kind,
                style = MaterialTheme.typography.titleSmall,
                color = Color(0xFFF2F4F8),
            )

            val dir = camera.directionDeg
            Text(
                if (dir != null) {
                    "Faces ${dir.roundToInt()}° (${compassPoint(dir)})"
                } else {
                    "No direction mapped — treated as covering all round"
                },
                style = MaterialTheme.typography.bodySmall,
                color = Color(0xFFC7D0DA),
            )
            Text(
                if (dir != null && camera.kind == CameraKind.FIXED) {
                    "Modelled cover: ${camera.rangeM.roundToInt()} m, " +
                        "${(camera.halfFovDeg * 2).roundToInt()}° wide"
                } else {
                    "Modelled cover: ${camera.rangeM.roundToInt()} m in every direction"
                },
                style = MaterialTheme.typography.bodySmall,
                color = Color(0xFFC7D0DA),
            )
            Text(
                "Range and field of view are this app's assumptions, not OSM " +
                    "data. OSM node ${camera.osmId}.",
                style = MaterialTheme.typography.labelSmall,
                color = Color(0xFF9AA4B2),
            )
            Text(
                "Dismiss",
                style = MaterialTheme.typography.labelLarge,
                color = Color(0xFF7FD4A2),
                modifier = Modifier
                    .clickable(onClick = onDismiss)
                    .padding(top = 4.dp),
            )
        }
    }
}

@Composable
private fun StatusCard(state: RouteViewModel.UiState, modifier: Modifier = Modifier) {
    val text = when (state) {
        is RouteViewModel.UiState.Loading -> "Loading Berlin surveillance map…"

        is RouteViewModel.UiState.Ready ->
            "${state.cameraCount} mapped cameras. Tap a start, then a destination."

        is RouteViewModel.UiState.Planning -> "Planning the quiet way…"

        is RouteViewModel.UiState.Routed ->
            "${state.route.lengthM.toInt()} m · " +
                "${(state.route.meanExposure * 100).toInt()}% under watch"

        is RouteViewModel.UiState.Error -> state.message
    }
    Card(
        modifier,
        colors = CardDefaults.cardColors(containerColor = Color(0xE6161B22)),
    ) {
        Text(
            text,
            Modifier.padding(12.dp),
            style = MaterialTheme.typography.bodyMedium,
            color = Color(0xFFF2F4F8),
        )
    }
}

/** How far around the viewport centre to pull cameras for the map layer. */
private const val TAG = "Schattenweg"

/**
 * How far to ask for cameras, derived from what is actually on screen.
 *
 * A fixed radius means zooming out shows more map but no more cameras --
 * they stop in a disc around the centre. Berlin's extract is a few thousand
 * cameras, so asking for the whole viewport is cheap; the bounds keep a
 * degenerate projection from asking for a metre or for the planet.
 */
private fun MapLibreMap.viewportRadiusM(): Double {
    val centre = cameraPosition.target ?: return 2_000.0
    val bounds = projection.visibleRegion.latLngBounds
    // LatLngBounds is Kotlin, so these are public fields; getLatNorth()/
    // getLonEast() are plain functions, not synthesised properties.
    val corner = LatLng(bounds.latitudeNorth, bounds.longitudeEast)
    return centre.distanceTo(corner).coerceIn(500.0, 40_000.0)
}

/**
 * Above this many cameras the coverage wedges are sub-pixel clutter and cost
 * more to build than they convey, so only the dots are drawn. The fill layer
 * also carries a minzoom for the same reason.
 */
private const val MAX_COVERAGE_FEATURES = 1_500

private const val EMPTY_COLLECTION = """{"type":"FeatureCollection","features":[]}"""

private fun camerasGeoJson(cameras: List<Camera>): String {
    if (cameras.isEmpty()) return EMPTY_COLLECTION
    // osm_id is carried so a tap can be resolved back to the camera it hit.
    val features = cameras.joinToString(",") { c ->
        """{"type":"Feature","geometry":{"type":"Point",""" +
            """"coordinates":[${c.lon},${c.lat}]},""" +
            """"properties":{"osm_id":${c.osmId}}}"""
    }
    return """{"type":"FeatureCollection","features":[$features]}"""
}

/**
 * The coverage the router actually models, as polygons: a wedge for a fixed
 * camera with a known bearing, a disc for anything that can point about
 * freely or has no direction mapped. This is deliberately the same rule as
 * `camera.rs`, so what you see is what the exposure score used -- including
 * the fact that range and field of view are assumptions, not OSM data.
 */
private fun coverageGeoJson(cameras: List<Camera>): String {
    if (cameras.isEmpty() || cameras.size > MAX_COVERAGE_FEATURES) return EMPTY_COLLECTION
    val features = cameras.joinToString(",") { c ->
        val dir = c.directionDeg
        val ring = if (c.kind == CameraKind.FIXED && dir != null) {
            wedgeRing(c.lat, c.lon, dir, c.halfFovDeg, c.rangeM)
        } else {
            discRing(c.lat, c.lon, c.rangeM)
        }
        val coords = ring.joinToString(",") { (lat, lon) -> "[$lon,$lat]" }
        """{"type":"Feature","geometry":{"type":"Polygon",""" +
            """"coordinates":[[$coords]]},"properties":{}}"""
    }
    return """{"type":"FeatureCollection","features":[$features]}"""
}

/** Metres offset to a lat/lon. Flat-earth is fine over a few tens of metres. */
private fun offsetMetres(lat: Double, lon: Double, bearingDeg: Double, distM: Double):
    Pair<Double, Double> {
    val br = Math.toRadians(bearingDeg)
    val dLat = distM * cos(br) / 111_320.0
    val dLon = distM * sin(br) / (111_320.0 * cos(Math.toRadians(lat)))
    return (lat + dLat) to (lon + dLon)
}

private fun wedgeRing(
    lat: Double,
    lon: Double,
    dirDeg: Double,
    halfFovDeg: Double,
    rangeM: Double,
    steps: Int = 12,
): List<Pair<Double, Double>> {
    val ring = mutableListOf(lat to lon)
    for (i in 0..steps) {
        val bearing = dirDeg - halfFovDeg + (2 * halfFovDeg) * i / steps
        ring += offsetMetres(lat, lon, bearing, rangeM)
    }
    ring += lat to lon
    return ring
}

private fun discRing(
    lat: Double,
    lon: Double,
    rangeM: Double,
    steps: Int = 24,
): List<Pair<Double, Double>> {
    val ring = (0 until steps).map { i ->
        offsetMetres(lat, lon, 360.0 * i / steps, rangeM)
    }
    return ring + ring.first()
}

/** 0 = N, 90 = E. */
private fun compassPoint(deg: Double): String {
    val names = listOf("N", "NE", "E", "SE", "S", "SW", "W", "NW")
    val norm = ((deg % 360.0) + 360.0) % 360.0
    return names[(norm / 45.0).roundToInt() % 8]
}

private fun pointsGeoJson(points: List<LatLon>): String {
    if (points.isEmpty()) return EMPTY_COLLECTION
    val features = points.joinToString(",") {
        """{"type":"Feature","geometry":{"type":"Point","coordinates":[${it.lon},${it.lat}]}}"""
    }
    return """{"type":"FeatureCollection","features":[$features]}"""
}

private fun routeGeoJson(polyline: List<LatLon>?): String {
    if (polyline.isNullOrEmpty()) return EMPTY_COLLECTION
    val coords = polyline.joinToString(",") { "[${it.lon},${it.lat}]" }
    return """{"type":"FeatureCollection","features":[
        {"type":"Feature","geometry":{"type":"LineString","coordinates":[$coords]}}]}"""
}
