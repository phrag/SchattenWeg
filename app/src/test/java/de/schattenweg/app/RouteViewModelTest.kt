package de.schattenweg.app

import android.app.Application
import androidx.lifecycle.SavedStateHandle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import uniffi.schattenweg_core.Camera
import uniffi.schattenweg_core.CoverageShape
import uniffi.schattenweg_core.InternalException
import uniffi.schattenweg_core.LatLon
import uniffi.schattenweg_core.Place
import uniffi.schattenweg_core.Route
import uniffi.schattenweg_core.RouterInterface
import java.io.File
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * The ViewModel's rules, run on the JVM against a fake core: no device, no
 * native library. Core calls still run on the real Dispatchers.Default, so
 * results arrive on another thread and the tests wait for them.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class RouteViewModelTest {

    private val a = LatLon(52.4990, 13.4180)
    private val b = LatLon(52.5010, 13.4200)

    @Before
    fun setUp() = Dispatchers.setMain(UnconfinedTestDispatcher())

    @After
    fun tearDown() = Dispatchers.resetMain()

    /** Route length encodes the λ it was planned with: 1000 m + λ. */
    private class FakeRouter : RouterInterface {
        val lambdas: MutableList<Double> = Collections.synchronizedList(mutableListOf())
        var planFailure: Exception? = null

        /** When set, a plan at λ = 1 (Low) blocks until it is counted down. */
        var lowGate: CountDownLatch? = null
        val lowStarted = CountDownLatch(1)
        val lowFinished = CountDownLatch(1)

        override fun plan(start: LatLon, end: LatLon, lambda: Double): Route {
            lambdas += lambda
            planFailure?.let { throw it }
            if (lambda == AvoidanceLevel.LOW.lambda) {
                lowGate?.let { gate ->
                    lowStarted.countDown()
                    gate.await(5, TimeUnit.SECONDS)
                    lowFinished.countDown()
                }
            }
            return Route(listOf(start, end), 1000.0 + lambda, 0.0)
        }

        override fun cameraCount(): ULong = 3u
        override fun camerasNear(at: LatLon, radiusM: Double): List<Camera> = emptyList()
        override fun coverageNear(at: LatLon, radiusM: Double): List<CoverageShape> = emptyList()
        override fun placeCount(): ULong = 0u
        override fun searchPlaces(query: String, limit: UInt): List<Place> = emptyList()
    }

    private fun viewModel(
        router: FakeRouter,
        saved: SavedStateHandle = SavedStateHandle(),
        assets: () -> MapAssets.Provisioned = {
            MapAssets.Provisioned(null, File("routing.graphcache"), null, null)
        },
    ) = RouteViewModel(Application(), saved).apply {
        provisionAssets = assets
        openRouter = { _, _ -> router }
    }

    private fun waitFor(what: String, condition: () -> Boolean) {
        repeat(500) {
            if (condition()) return
            Thread.sleep(10)
        }
        fail("timed out waiting for $what")
    }

    private fun RouteViewModel.ready() {
        provision()
        waitFor("router") { state.value is RouteViewModel.UiState.Ready }
    }

    @Test
    fun fullDiskShowsAnErrorInsteadOfCrashing() {
        val vm = viewModel(FakeRouter(), assets = {
            throw MapAssets.ProvisionException("Not enough free storage")
        })
        vm.provision()
        waitFor("error state") { vm.state.value is RouteViewModel.UiState.Error }
        assertEquals(RouteViewModel.UiState.Error("Not enough free storage"), vm.state.value)
        assertTrue("basemap must not wait forever", vm.basemapReady.value)
    }

    @Test
    fun aPanicInTheCoreIsReportedNotThrown() {
        val router = FakeRouter().apply { planFailure = InternalException("panicked at routing.rs") }
        val vm = viewModel(router)
        vm.ready()
        vm.onMapTap(a)
        vm.onMapTap(b)
        waitFor("error state") { vm.state.value is RouteViewModel.UiState.Error }
        assertNull(vm.route.value)
    }

    @Test
    fun aNewPairStartsAtHighButAManualLevelIsHonoured() {
        val router = FakeRouter()
        val vm = viewModel(router)
        vm.ready()
        vm.level.value = AvoidanceLevel.LOW
        vm.onMapTap(a)
        vm.onMapTap(b)
        waitFor("high route") { vm.route.value?.lengthM == 1006.0 }
        assertEquals(AvoidanceLevel.HIGH, vm.level.value)

        vm.level.value = AvoidanceLevel.MEDIUM
        vm.plan()
        waitFor("medium route") { vm.route.value?.lengthM == 1003.0 }
    }

    @Test
    fun aSlowStalePlanCannotReplaceTheLatestOne() {
        val router = FakeRouter()
        val vm = viewModel(router)
        vm.ready()
        vm.onMapTap(a)
        vm.onMapTap(b)
        waitFor("first route") { vm.route.value?.lengthM == 1006.0 }

        // Low is slow, then the user flips back to High before it returns.
        val gate = CountDownLatch(1)
        router.lowGate = gate
        vm.level.value = AvoidanceLevel.LOW
        vm.plan()
        assertTrue(router.lowStarted.await(5, TimeUnit.SECONDS))
        vm.level.value = AvoidanceLevel.HIGH
        vm.plan()
        waitFor("second high route") { vm.state.value is RouteViewModel.UiState.Routed }

        gate.countDown()
        assertTrue(router.lowFinished.await(5, TimeUnit.SECONDS))
        Thread.sleep(100) // let a (buggy) stale continuation land if it would
        assertEquals(1006.0, vm.route.value?.lengthM)
        assertEquals(AvoidanceLevel.HIGH, vm.level.value)
    }

    @Test
    fun pointsSurviveProcessDeathAndAreReplannedAtTheirLevel() {
        val saved = SavedStateHandle()
        val vm1 = viewModel(FakeRouter(), saved)
        vm1.ready()
        vm1.onMapTap(a)
        vm1.onMapTap(b)
        vm1.level.value = AvoidanceLevel.LOW

        // A new process: a fresh ViewModel over a copy of the saved state.
        val restored = SavedStateHandle(saved.keys().associateWith { saved.get<Any>(it) })
        val router = FakeRouter()
        val vm2 = viewModel(router, restored)
        assertEquals(a, vm2.start.value)
        assertEquals(b, vm2.end.value)
        vm2.provision()
        waitFor("restored route") { vm2.route.value != null }
        assertEquals(listOf(AvoidanceLevel.LOW.lambda), router.lambdas.toList())
    }
}
