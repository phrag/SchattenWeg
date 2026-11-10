package de.schattenweg.app

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.location.Location
import android.location.LocationListener
import android.location.LocationManager
import android.os.Build
import android.os.Bundle
import android.os.CancellationSignal
import android.os.Looper
import android.os.SystemClock
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull
import kotlin.coroutines.resume

/**
 * One-shot GPS position, on demand only.
 *
 * Privacy rules (CLAUDE.md §3), all enforced here:
 * - **GPS_PROVIDER only.** Never NETWORK_PROVIDER (Wi-Fi/cell lookups can go
 *   through a vendor's server) and never the Fused Location Provider (Google
 *   Play Services). On a phone with no GPS this simply reports
 *   [Outcome.NoGps].
 * - **One fix, then stop.** There is no tracking: each call asks for a single
 *   location and releases the provider as soon as it has one, times out, or
 *   the caller's coroutine is cancelled (the screen left the foreground).
 * - **Nothing is stored or logged.** The result goes back to the caller and
 *   nowhere else.
 */
internal object GpsFix {

    sealed interface Outcome {
        data class Fix(val lat: Double, val lon: Double, val accuracyM: Float?) : Outcome

        /** The device has no GPS receiver. */
        data object NoGps : Outcome

        /** Location (or GPS) is switched off in system settings. */
        data object LocationOff : Outcome

        /** Precise location is not granted (or was revoked mid-request). */
        data object NoPermission : Outcome

        /** No fix within the timeout -- typically indoors. */
        data object TimedOut : Outcome
    }

    /** A last GPS fix younger than this is reused instead of waking the receiver. */
    private const val FRESH_FIX_MS = 15_000L

    fun hasPrecisePermission(context: Context): Boolean =
        context.checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) ==
            PackageManager.PERMISSION_GRANTED

    fun hasApproximatePermission(context: Context): Boolean =
        context.checkSelfPermission(Manifest.permission.ACCESS_COARSE_LOCATION) ==
            PackageManager.PERMISSION_GRANTED

    /**
     * Wait up to [timeoutMs] for a single GPS fix. Cancelling the calling
     * coroutine cancels the platform request, so the receiver is released.
     */
    @SuppressLint("MissingPermission") // checked first; SecurityException caught
    suspend fun current(context: Context, timeoutMs: Long): Outcome {
        if (!hasPrecisePermission(context)) return Outcome.NoPermission
        val lm = context.getSystemService(LocationManager::class.java) ?: return Outcome.NoGps
        if (LocationManager.GPS_PROVIDER !in lm.allProviders) return Outcome.NoGps
        if (!lm.isLocationEnabled || !lm.isProviderEnabled(LocationManager.GPS_PROVIDER)) {
            return Outcome.LocationOff
        }
        return try {
            val recent = lm.getLastKnownLocation(LocationManager.GPS_PROVIDER)
                ?.takeIf { ageMs(it) in 0..FRESH_FIX_MS }
            val loc = recent ?: withTimeoutOrNull(timeoutMs) { awaitFix(context, lm) }
            when {
                loc != null -> loc.toFix()
                !lm.isProviderEnabled(LocationManager.GPS_PROVIDER) -> Outcome.LocationOff
                else -> Outcome.TimedOut
            }
        } catch (e: SecurityException) {
            Outcome.NoPermission
        }
    }

    private fun ageMs(loc: Location): Long =
        (SystemClock.elapsedRealtimeNanos() - loc.elapsedRealtimeNanos) / 1_000_000

    private fun Location.toFix() =
        Outcome.Fix(latitude, longitude, if (hasAccuracy()) accuracy else null)

    /** One fix from GPS_PROVIDER, or null if the platform gave up. */
    @SuppressLint("MissingPermission")
    private suspend fun awaitFix(context: Context, lm: LocationManager): Location? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            suspendCancellableCoroutine { cont ->
                val signal = CancellationSignal()
                cont.invokeOnCancellation { signal.cancel() }
                lm.getCurrentLocation(
                    LocationManager.GPS_PROVIDER,
                    signal,
                    context.mainExecutor,
                ) { loc -> if (cont.isActive) cont.resume(loc) }
            }
        } else {
            // API 29: no getCurrentLocation. Listen, take the first fix, and
            // unregister. All four callbacks are overridden because on API 29
            // they are abstract, and the platform calls them.
            suspendCancellableCoroutine { cont ->
                val listener = object : LocationListener {
                    override fun onLocationChanged(location: Location) {
                        lm.removeUpdates(this)
                        if (cont.isActive) cont.resume(location)
                    }

                    override fun onProviderDisabled(provider: String) {
                        lm.removeUpdates(this)
                        if (cont.isActive) cont.resume(null)
                    }

                    override fun onProviderEnabled(provider: String) = Unit

                    @Deprecated("Required on API 29, where it is abstract")
                    override fun onStatusChanged(provider: String?, status: Int, extras: Bundle?) =
                        Unit
                }
                cont.invokeOnCancellation { lm.removeUpdates(listener) }
                lm.requestLocationUpdates(
                    LocationManager.GPS_PROVIDER,
                    0L,
                    0f,
                    listener,
                    Looper.getMainLooper(),
                )
            }
        }
}
