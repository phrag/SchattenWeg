package de.schattenweg.app

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
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
class RouteViewModel : ViewModel() {

    private var router: Router? = null

    private val _state = MutableStateFlow<UiState>(UiState.Loading)
    val state: StateFlow<UiState> = _state.asStateFlow()

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
     * Build the core from a Berlin `.osm.pbf` already on disk (bundled asset
     * copied into app storage). Heavy — do it once, off-thread.
     */
    fun initialise(pbfPath: String?) {
        if (pbfPath == null) {
            _state.value = UiState.Error(
                "No Berlin map data bundled. Run scripts/build_map_assets.sh and rebuild.",
            )
            return
        }
        viewModelScope.launch {
            _state.value = UiState.Loading
            _state.value = try {
                val r = withContext(Dispatchers.Default) { Router.fromPbf(pbfPath) }
                router = r
                UiState.Ready(cameraCount = r.cameraCount())
            } catch (e: RouteException) {
                UiState.Error(e.message ?: "Could not load the map data")
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
                UiState.Error(ex.message ?: "Could not plan a route")
            }
        }
    }

    /** Refresh the camera layer for the visible region. */
    fun refreshCameras(centre: LatLon, radiusM: Double) {
        val r = router ?: return
        viewModelScope.launch {
            cameras.value = withContext(Dispatchers.Default) { r.camerasNear(centre, radiusM) }
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
