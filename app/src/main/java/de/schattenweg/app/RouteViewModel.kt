package de.schattenweg.app

import android.app.Application
import android.util.Log
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.Job
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
import uniffi.schattenweg_core.RouterInterface
import java.io.File

/**
 * Owns the [Router] (the Rust core) and exposes route-planning state to the UI.
 *
 * Everything that touches the core runs off the main thread. The core is
 * immutable after construction, so a single instance is shared for the app's
 * lifetime.
 *
 * Every call into the core is guarded: besides [RouteException], a Rust panic
 * surfaces as UniFFI's `InternalException`, and an uncaught exception in
 * [viewModelScope] kills the process. A failed call shows an error instead.
 */
class RouteViewModel(
    application: Application,
    private val saved: SavedStateHandle,
) : AndroidViewModel(application) {

    private var router: RouterInterface? = null

    /**
     * Seams for unit tests (`RouteViewModelTest`): the real app copies the
     * bundled assets out and opens the native core; tests substitute fakes so
     * the ViewModel's rules run on the JVM without Android or the Rust library.
     */
    internal var provisionAssets: () -> MapAssets.Provisioned =
        { MapAssets.ensure(getApplication<Application>()) }
    internal var openRouter: (pbf: File?, cache: File) -> RouterInterface = { pbf, cache ->
        if (pbf != null) {
            // Only an extract was bundled: open() reuses a valid cached scored
            // graph, and otherwise scores one from the extract and writes it
            // for next time.
            Router.open(pbf.absolutePath, cache.absolutePath)
        } else {
            // The normal build: the bundled cache is the routing data, and
            // there is no extract to fall back to.
            Router.openCache(cache.absolutePath)
        }
    }

    /** The plan in flight; a newer request cancels it so a stale result can't win. */
    private var planJob: Job? = null

    /** Same for the camera layer: only the latest viewport's answer is drawn. */
    private var cameraJob: Job? = null

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

    init {
        // Android may kill the process while the app is in the background; the
        // ViewModel then starts empty. Bring back the dropped points and the
        // level, and re-plan once the router is ready (see provision()). Only
        // the user's own taps are kept, in the platform's per-task saved state,
        // which never leaves the device and is discarded when the task ends.
        start.value = saved.get<DoubleArray>(KEY_START)?.toLatLon()
        end.value = saved.get<DoubleArray>(KEY_END)?.toLatLon()
        saved.get<String>(KEY_LEVEL)
            ?.let { name -> AvoidanceLevel.entries.firstOrNull { it.name == name } }
            ?.let { level.value = it }
        viewModelScope.launch { start.collect { saved[KEY_START] = it?.toArray() } }
        viewModelScope.launch { end.collect { saved[KEY_END] = it?.toArray() } }
        viewModelScope.launch { level.collect { saved[KEY_LEVEL] = it.name } }
        // Once per ViewModel, not per provision(): a retried provision after a
        // failure must not start a second collector.
        startSearchCollector()
    }

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
        viewModelScope.launch {
            _state.value = UiState.Loading

            val assets = try {
                withContext(Dispatchers.IO) { provisionAssets() }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                // Typically a full disk: ~200 MB is copied out on first launch.
                // Leave the basemap blank, say why, and allow a retry the next
                // time the activity is created.
                Log.w(TAG, "map assets could not be provisioned", e)
                basemapReady.value = true
                provisioning = false
                _state.value = UiState.Error(
                    (e as? MapAssets.ProvisionException)?.message
                        ?: "Could not unpack the map data. " +
                        "Free up some storage and reopen the app.",
                )
                return@launch
            }
            mapAssets.value = assets
            basemapReady.value = true

            val pbf = assets.routingPbf
            val cache = assets.routingCache
            if (cache == null) {
                _state.value = UiState.Error(
                    "No Berlin map data bundled. Run scripts/build_map_assets.sh and rebuild.",
                )
                return@launch
            }

            _state.value = try {
                val r = withContext(Dispatchers.Default) { openRouter(pbf, cache) }
                router = r
                // The map almost certainly asked for cameras while this was
                // still loading; answer that request now.
                lastCameraQuery?.let { (centre, radiusM) ->
                    refreshCameras(centre, radiusM)
                }
                UiState.Ready(cameraCount = r.cameraCount())
            } catch (e: CancellationException) {
                throw e
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
            // Points restored after process death, or tapped while the router
            // was still loading: plan them now, at the level that is showing
            // (not reset to High: for a restored pair it is the same route).
            if (router != null && start.value != null && end.value != null) plan()
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
        val lambda = level.value.lambda
        // A quick Low → High flip starts two plans; without this the slower
        // one could land last and show the route for the level not selected.
        // (The native call itself runs to completion; its result is dropped.)
        planJob?.cancel()
        planJob = viewModelScope.launch {
            _state.value = UiState.Planning
            _state.value = try {
                val planned = withContext(Dispatchers.Default) { r.plan(s, e, lambda) }
                route.value = planned
                UiState.Routed(planned)
            } catch (ex: CancellationException) {
                throw ex
            } catch (ex: RouteException) {
                route.value = null
                UiState.Error(ex.explain())
            } catch (ex: Exception) {
                // A Rust panic (UniFFI InternalException): report, don't crash.
                Log.e(TAG, "route planning failed", ex)
                route.value = null
                UiState.Error("Route planning failed. Try other points.")
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
        cameraJob?.cancel()
        cameraJob = viewModelScope.launch {
            val (found, shapes) = try {
                withContext(Dispatchers.Default) {
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
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                // Keep the layer as it was; the next pan asks again.
                Log.e(TAG, "camera query failed", e)
                return@launch
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
                    searchResults.value = try {
                        withContext(Dispatchers.Default) { r.searchPlaces(q, 8u) }
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        // An exception here would end the collector, and search
                        // would silently stop working for the rest of the session.
                        Log.e(TAG, "place search failed", e)
                        emptyList()
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
        (router as? AutoCloseable)?.close()
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

private const val TAG = "SchattenWeg"

private const val KEY_START = "start"
private const val KEY_END = "end"
private const val KEY_LEVEL = "level"

private fun LatLon.toArray() = doubleArrayOf(lat, lon)

private fun DoubleArray.toLatLon(): LatLon? = if (size == 2) LatLon(this[0], this[1]) else null
