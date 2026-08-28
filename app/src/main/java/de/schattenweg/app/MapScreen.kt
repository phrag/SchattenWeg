package de.schattenweg.app

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.viewmodel.compose.viewModel
import org.maplibre.android.MapLibre
import org.maplibre.android.camera.CameraPosition
import org.maplibre.android.geometry.LatLng
import org.maplibre.android.maps.MapView
import org.maplibre.android.maps.Style
import org.maplibre.android.style.layers.CircleLayer
import org.maplibre.android.style.layers.LineLayer
import org.maplibre.android.style.layers.PropertyFactory
import org.maplibre.android.style.sources.GeoJsonSource
import uniffi.schattenweg_core.LatLon

private const val CAMERA_SOURCE = "sw-cameras"
private const val ROUTE_SOURCE = "sw-route"
private const val ENDPOINT_SOURCE = "sw-endpoints"

/** Berlin, Alexanderplatz — where the map opens. */
private val BERLIN = LatLng(52.5216, 13.4127)

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
    val lambda by viewModel.lambda.collectAsState()
    val cameras by viewModel.cameras.collectAsState()
    val route by viewModel.route.collectAsState()
    val start by viewModel.start.collectAsState()
    val end by viewModel.end.collectAsState()

    val assets = remember { MapAssets.ensure(context) }
    val mapView = remember {
        MapLibre.getInstance(context)
        MapView(context)
    }

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
                Lifecycle.Event.ON_DESTROY -> mapView.onDestroy()
                else -> Unit
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
            mapView.onDestroy()
        }
    }

    Box(Modifier.fillMaxSize()) {

        AndroidView(
            factory = {
                mapView.getMapAsync { map ->
                    map.cameraPosition = CameraPosition.Builder()
                        .target(BERLIN)
                        .zoom(14.0)
                        .build()

                    map.setStyle(
                        Style.Builder().fromJson(MapAssets.styleJson(context, assets.pmtiles)),
                    ) { style ->
                        style.addSource(GeoJsonSource(ROUTE_SOURCE, EMPTY_COLLECTION))
                        style.addSource(GeoJsonSource(CAMERA_SOURCE, EMPTY_COLLECTION))
                        style.addSource(GeoJsonSource(ENDPOINT_SOURCE, EMPTY_COLLECTION))

                        style.addLayer(
                            LineLayer("sw-route-line", ROUTE_SOURCE).withProperties(
                                PropertyFactory.lineColor("#7fd4a2"),
                                PropertyFactory.lineWidth(5f),
                                PropertyFactory.lineCap("round"),
                                PropertyFactory.lineJoin("round"),
                            ),
                        )
                        style.addLayer(
                            CircleLayer("sw-camera-dots", CAMERA_SOURCE).withProperties(
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

                        val centre = map.cameraPosition.target
                        if (centre != null) {
                            viewModel.refreshCameras(
                                LatLon(centre.latitude, centre.longitude),
                                CAMERA_QUERY_RADIUS_M,
                            )
                        }
                    }

                    map.addOnMapClickListener { point ->
                        viewModel.onMapTap(LatLon(point.latitude, point.longitude))
                        true
                    }

                    map.addOnCameraIdleListener {
                        val centre = map.cameraPosition.target ?: return@addOnCameraIdleListener
                        viewModel.refreshCameras(
                            LatLon(centre.latitude, centre.longitude),
                            CAMERA_QUERY_RADIUS_M,
                        )
                    }
                }
                mapView
            },
            modifier = Modifier.fillMaxSize(),
            update = {
                mapView.getMapAsync { map ->
                    val style = map.style ?: return@getMapAsync
                    style.getSourceAs<GeoJsonSource>(CAMERA_SOURCE)
                        ?.setGeoJson(camerasGeoJson(cameras))
                    style.getSourceAs<GeoJsonSource>(ROUTE_SOURCE)
                        ?.setGeoJson(routeGeoJson(route?.polyline))
                    style.getSourceAs<GeoJsonSource>(ENDPOINT_SOURCE)
                        ?.setGeoJson(pointsGeoJson(listOfNotNull(start, end)))
                }
            },
        )

        StatusCard(
            state = state,
            modifier = Modifier
                .align(Alignment.TopCenter)
                .fillMaxWidth()
                .padding(12.dp),
        )

        // Paranoia dial + the two honesty notes (see CLAUDE.md §5 — these are
        // non-negotiable).
        Card(
            Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .padding(12.dp),
            colors = CardDefaults.cardColors(containerColor = Color(0xE6161B22)),
        ) {
            Column(
                Modifier.padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Text(
                    "Camera avoidance: ${"%.1f".format(lambda)}",
                    style = MaterialTheme.typography.titleSmall,
                    color = Color(0xFFF2F4F8),
                )
                Slider(
                    value = lambda.toFloat(),
                    onValueChange = { viewModel.lambda.value = it.toDouble() },
                    onValueChangeFinished = { viewModel.plan() },
                    valueRange = 0f..8f,
                )
                Text(
                    "Shows only cameras mapped in OpenStreetMap — real coverage " +
                        "is higher. Avoiding them is not anonymity.",
                    style = MaterialTheme.typography.bodySmall,
                    color = Color(0xFF9AA4B2),
                )
            }
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
private const val CAMERA_QUERY_RADIUS_M = 2_000.0

private const val EMPTY_COLLECTION = """{"type":"FeatureCollection","features":[]}"""

private fun camerasGeoJson(cameras: List<uniffi.schattenweg_core.Camera>): String =
    pointsGeoJson(cameras.map { LatLon(it.lat, it.lon) })

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
