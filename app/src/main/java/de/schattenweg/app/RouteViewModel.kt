package de.schattenweg.app

import android.app.Application
import android.util.Log
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.schattenweg_core.Camera
import uniffi.schattenweg_core.CoverageShape
import uniffi.schattenweg_core.LatLon
import uniffi.schattenweg_core.Place
import uniffi.schattenweg_core.Route
import uniffi.schattenweg_core.RouteException
import uniffi.schattenweg_core.Router

/**
 * Owns the [Router] (the Rust core) and exposes route-planning state to the UI.
 *
 * Everything that touches the core runs off the main thread. The core is
 * immutable after construction, so a single instance is shared for the app's
 * lifetime.
 */
class RouteViewModel(application: Application) : AndroidViewModel(application) {

    private var router: Router? = null

    /** Last region the map asked about, replayed when the router is ready. */
    private var lastCameraQuery: Pair<LatLon, Double>? = null
    private var provisioning = false

    private val _state = MutableStateFlow<UiState>(UiState.Loading)
    val state: StateFlow<UiState> = _state.asStateFlow()

    /**
     * The provisioned map assets (tiles + glyphs) once copied out, or null
     * while still copying / when none are bundled. The map style is built
     * from this.
     */
    val mapAssets = MutableStateFlow<MapAssets.Provisioned?>(null)

    /** True once [provision] has finished, whatever the outcome. */
    val basemapReady = MutableStateFlow(false)

    /**
     * Camera-avoidance strength, bound to a three-way control in the UI. Each
     * level is a fixed λ into the router's `length * (1 + λ * exposure)` cost
     * (see CLAUDE.md §4): [AvoidanceLevel.LOW] barely detours, [AvoidanceLevel.HIGH]
     * takes big detours to dodge lenses. Starts at [AvoidanceLevel.HIGH], and
     * every new route is reset to HIGH again (see [plan]); a manual pick only
     * applies to the route it was made on.
     */
    val level = MutableStateFlow(AvoidanceLevel.HIGH)

    /** Tapped start point, then destination; a third tap starts over. */
    val start = MutableStateFlow<LatLon?>(null)
    val end = MutableStateFlow<LatLon?>(null)

    /** Cameras to draw for the current viewport. */
    val cameras = MutableStateFlow<List<Camera>>(emptyList())

    /**
     * What those cameras can see: each outline already cut short where a
     * building blocks the view, computed by the core from the same geometry the
     * exposure score used. Empty when too many cameras are in view to draw.
     */
    val coverage = MutableStateFlow<List<CoverageShape>>(emptyList())

    /** The most recent successfully planned route, or null. */
    val route = MutableStateFlow<Route?>(null)

    /** The search box text, and the results for it. */
    val searchQuery = MutableStateFlow("")
    val searchResults = MutableStateFlow<List<Place>>(emptyList())

    /**
     * Copy the bundled map data out of the APK and build the Rust core from
     * it. Both steps are heavy — the snapshot is tens of megabytes and the
     * exposure pass walks every edge — so all of it runs off the main thread.
     * Safe to call repeatedly; the copy is skipped once the files are in place.
     */
    fun provision() {
        // Guard against a second pass: onCreate runs again on configuration
        // change while this ViewModel (and any in-flight load) survives.
        if (router != null || provisioning) return
        provisioning = true
        startSearchCollector()
        viewModelScope.launch {
            _state.value = UiState.Loading

            val assets = withContext(Dispatchers.IO) {
                MapAssets.ensure(getApplication<Application>())
            }
            mapAssets.value = assets
            basemapReady.value = true

            val pbf = assets.routingPbf
            if (pbf == null) {
                _state.value = UiState.Error(
                    "No Berlin map data bundled. Run scripts/build_map_assets.sh and rebuild.",
                )
                return@launch
            }

            _state.value = try {
                // open() reuses a cached scored graph when one is valid,
                // skipping the several-second exposure pass on every cold start
                // after the first. The cache lives beside the snapshot in
                // private storage; on a miss the core rebuilds and rewrites it.
                val cachePath = assets.routingCache!!.absolutePath
                val r = withContext(Dispatchers.Default) {
                    Router.open(pbf.absolutePath, cachePath)
                }
                router = r
                // The map almost certainly asked for cameras while this was
                // still loading; answer that request now.
                lastCameraQuery?.let { (centre, radiusM) ->
                    refreshCameras(centre, radiusM)
                }
                UiState.Ready(cameraCount = r.cameraCount())
            } catch (e: Exception) {
                // Deliberately broad: this is the one place that feeds a whole
                // file into the core. Besides RouteException, a malformed
                // extract can surface as UniFFI's InternalException (a Rust
                // panic), and telling the user beats killing the process.
                UiState.Error(
                    (e as? RouteException)?.explain()
                        ?: e.message?.takeIf { it.isNotBlank() }
                        ?: "Could not load the map data",
                )
            }
        }
    }

    /**
     * Wording a user can act on.
     *
     * UniFFI gives a fieldless error variant `message == ""` rather than null,
     * so an `?: fallback` never fires and the UI would show an empty error.
     * Matching on the variant also lets each case say what to do next, which
     * the Rust-side text does not.
     */
    private fun RouteException.explain(): String = when (this) {
        is RouteException.NoNearbyNode ->
            "No mapped street near there. Tap closer to a road."

        is RouteException.Unreachable ->
            "No walking route between those two points."

        is RouteException.LoadFailed ->
            "Could not load the map data: $reason"
    }

    /**
     * Undo the most recently dropped point: the destination first (leaving just
     * the start), then the start. Bound to a long-press on empty map — a
     * deliberate gesture, so it won't fire from stray taps — and a no-op when
     * nothing is dropped. Clearing the destination also drops the planned route
     * and returns to the Ready state.
     */
    fun clearLastPoint() {
        when {
            end.value != null -> {
                end.value = null
                route.value = null
                _state.value = UiState.Ready(router?.cameraCount() ?: 0uL)
            }

            start.value != null -> {
                start.value = null
                _state.value = UiState.Ready(router?.cameraCount() ?: 0uL)
            }
        }
    }

    /** Handle a map tap: first sets the start, second the destination. */
    fun onMapTap(point: LatLon) {
        if (start.value == null || end.value != null) {
            start.value = point
            end.value = null
            route.value = null
            (state.value as? UiState.Routed)?.let {
                _state.value = UiState.Ready(router?.cameraCount() ?: 0uL)
            }
        } else {
            end.value = point
            plan(freshPair = true)
        }
    }

    /**
     * Plan (or re-plan) between the two taps at the current [level].
     *
     * When [freshPair] is set — a newly completed A→B pair, not a manual level
     * change — [level] is first reset to [AvoidanceLevel.HIGH], whatever was
     * picked for the previous route (so a level chosen before the destination
     * exists is overwritten too). The rule: every new route starts at the
     * strongest avoidance, which also gives a camera-free route whenever one
     * exists. A manual level change passes `freshPair = false`, so Low/Medium
     * are honoured exactly for that route even when a longer camera-free one
     * exists.
     */
    fun plan(freshPair: Boolean = false) {
        val r = router ?: return
        val s = start.value ?: return
        val e = end.value ?: return
        if (freshPair) level.value = AvoidanceLevel.HIGH
        viewModelScope.launch {
            _state.value = UiState.Planning
            _state.value = try {
                val planned = withContext(Dispatchers.Default) {
                    r.plan(s, e, level.value.lambda)
                }
                route.value = planned
                UiState.Routed(planned)
            } catch (ex: RouteException) {
                route.value = null
                UiState.Error(ex.explain())
            }
        }
    }

    /**
     * Refresh the camera layer for the visible region.
     *
     * The map is ready long before the router is: loading the Berlin extract
     * and scoring exposure takes seconds, while the style loads in
     * milliseconds. A request that arrives in that window is remembered and
     * replayed once the router exists -- otherwise the layer stays empty
     * until something moves the map, which on a fresh launch is never.
     */
    fun refreshCameras(centre: LatLon, radiusM: Double) {
        lastCameraQuery = centre to radiusM
        val r = router ?: return
        viewModelScope.launch {
            val (found, shapes) = withContext(Dispatchers.Default) {
                val near = r.camerasNear(centre, radiusM)
                // Past the draw cap the outlines are never shown, so don't
                // spend the ray-casting on them.
                val outlines = if (near.size <= MAX_COVERAGE_FEATURES) {
                    r.coverageNear(centre, radiusM)
                } else {
                    emptyList()
                }
                near to outlines
            }
            Log.d(
                TAG,
                "cameras within ${radiusM.toInt()} m of " +
                    "${centre.lat},${centre.lon}: ${found.size}",
            )
            cameras.value = found
            coverage.value = shapes
        }
    }

    /**
     * Run the place search off the main thread as the query changes, debounced
     * so a fast typist does not fire a scan per keystroke. There is no network
     * here -- search_places reads the bundled index, so this leaks nothing.
     */
    @OptIn(FlowPreview::class)
    private fun startSearchCollector() {
        viewModelScope.launch {
            searchQuery
                .debounce(180)
                .distinctUntilChanged()
                .collect { q ->
                    val r = router
                    if (r == null || q.isBlank()) {
                        searchResults.value = emptyList()
                        return@collect
                    }
                    searchResults.value = withContext(Dispatchers.Default) {
                        r.searchPlaces(q, 8u)
                    }
                }
        }
    }

    /** Set the start from a search result; re-plan if a destination exists. */
    fun setStart(point: LatLon) {
        start.value = point
        if (end.value != null) plan(freshPair = true)
    }

    /** Set the destination from a search result; re-plan if a start exists. */
    fun setEnd(point: LatLon) {
        end.value = point
        if (start.value != null) plan(freshPair = true)
    }

    /** Clear the search box and its results (e.g. after picking a result). */
    fun clearSearch() {
        searchQuery.value = ""
        searchResults.value = emptyList()
    }

    override fun onCleared() {
        // Router holds native memory; close it so UniFFI frees it promptly.
        router?.close()
        router = null
        super.onCleared()
    }

    sealed interface UiState {
        data object Loading : UiState
        data class Ready(val cameraCount: ULong) : UiState
        data object Planning : UiState
        data class Routed(val route: Route) : UiState
        data class Error(val message: String) : UiState
    }
}

/**
 * The three camera-avoidance settings the UI offers, and the λ each feeds to
 * [Router.plan]. λ scales the exposure penalty in the edge cost
 * `length_m * (1 + λ * exposure)`: LOW barely detours, HIGH takes long detours
 * to dodge cameras. These values replace the old free 0–8 slider — three named
 * choices are easier to reason about than a bare number, and the router treats
 * λ continuously so the exact values are just sensible presets (CLAUDE.md §4).
 */
enum class AvoidanceLevel(val label: String, val lambda: Double) {
    LOW("Low", 1.0),
    MEDIUM("Medium", 3.0),
    HIGH("High", 6.0),
}

private const val TAG = "Schattenweg"
