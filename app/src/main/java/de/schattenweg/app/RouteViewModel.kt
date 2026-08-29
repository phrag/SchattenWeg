package de.schattenweg.app

import android.app.Application
import android.util.Log
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.schattenweg_core.Camera
import uniffi.schattenweg_core.LatLon
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
     * Path to the offline basemap once provisioned, or null while still
     * copying / when no tiles are bundled. The map style is built from this.
     */
    val basemap = MutableStateFlow<File?>(null)

    /** True once [provision] has finished, whatever the outcome. */
    val basemapReady = MutableStateFlow(false)

    /** Paranoia dial, bound to a slider in the UI. 0 = shortest, higher = shyer. */
    val lambda = MutableStateFlow(2.0)

    /** Tapped start point, then destination; a third tap starts over. */
    val start = MutableStateFlow<LatLon?>(null)
    val end = MutableStateFlow<LatLon?>(null)

    /** Cameras to draw for the current viewport. */
    val cameras = MutableStateFlow<List<Camera>>(emptyList())

    /** The most recent successfully planned route, or null. */
    val route = MutableStateFlow<Route?>(null)

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

            val assets = withContext(Dispatchers.IO) {
                MapAssets.ensure(getApplication<Application>())
            }
            basemap.value = assets.pmtiles
            basemapReady.value = true

            val pbf = assets.routingPbf
            if (pbf == null) {
                _state.value = UiState.Error(
                    "No Berlin map data bundled. Run scripts/build_map_assets.sh and rebuild.",
                )
                return@launch
            }

            _state.value = try {
                val r = withContext(Dispatchers.Default) { Router.fromPbf(pbf.absolutePath) }
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
            plan()
        }
    }

    /** Plan (or re-plan, e.g. after the slider moves) between the two taps. */
    fun plan() {
        val r = router ?: return
        val s = start.value ?: return
        val e = end.value ?: return
        viewModelScope.launch {
            _state.value = UiState.Planning
            _state.value = try {
                val planned = withContext(Dispatchers.Default) { r.plan(s, e, lambda.value) }
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
            val found = withContext(Dispatchers.Default) { r.camerasNear(centre, radiusM) }
            Log.d(TAG, "cameras within ${radiusM.toInt()} m of " +
                "${centre.lat},${centre.lon}: ${found.size}")
            cameras.value = found
        }
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

private const val TAG = "Schattenweg"
