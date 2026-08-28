package de.schattenweg.app

import android.content.Context
import java.io.File

/**
 * Provisions the bundled map data into app-private storage.
 *
 * Both the routing snapshot (read by the Rust core) and the PMTiles basemap
 * (read by MapLibre, which needs byte-range access that Android assets can't
 * provide) are copied from APK assets to `filesDir/map/` on first launch.
 *
 * The assets are produced by `scripts/build_map_assets.sh`; a build without
 * them still runs (routes unavailable / plain background) so CI can assemble
 * the APK without the ~100 MB of Berlin data.
 */
object MapAssets {

    data class Provisioned(
        /** Berlin routing snapshot for the Rust core, or null if not bundled. */
        val routingPbf: File?,
        /** Offline basemap tiles, or null if not bundled. */
        val pmtiles: File?,
    )

    private const val ASSET_DIR = "map"
    private const val ROUTING_ASSET = "berlin-routing.osm.pbf"
    private const val TILES_ASSET = "berlin.pmtiles"

    fun ensure(context: Context): Provisioned {
        val bundled = context.assets.list(ASSET_DIR)?.toSet() ?: emptySet()
        val outDir = File(context.filesDir, "map").apply { mkdirs() }
        return Provisioned(
            routingPbf = copyIfBundled(context, bundled, ROUTING_ASSET, outDir),
            pmtiles = copyIfBundled(context, bundled, TILES_ASSET, outDir),
        )
    }

    private fun copyIfBundled(
        context: Context,
        bundled: Set<String>,
        name: String,
        outDir: File,
    ): File? {
        if (name !in bundled) return null
        val out = File(outDir, name)
        val assetSize = context.assets.openFd("$ASSET_DIR/$name").use { it.length }
        if (out.length() != assetSize) {
            context.assets.open("$ASSET_DIR/$name").use { input ->
                out.outputStream().use { input.copyTo(it) }
            }
        }
        return out
    }

    /** The MapLibre style JSON, pointing at the local tiles when present. */
    fun styleJson(context: Context, pmtiles: File?): String {
        if (pmtiles == null) {
            // No basemap bundled: bare dark canvas; cameras/route still render.
            return """{"version":8,"name":"fallback","sources":{},"layers":[
                {"id":"background","type":"background",
                 "paint":{"background-color":"#10141a"}}]}"""
        }
        val template = context.assets.open("style_template.json")
            .bufferedReader().use { it.readText() }
        return template.replace("__PMTILES_URL__", "file://${pmtiles.absolutePath}")
    }
}
