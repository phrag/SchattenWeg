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
        /** Directory holding the label glyph tree, or null if not bundled. */
        val glyphsDir: File?,
    )

    private const val ASSET_DIR = "map"
    private const val ROUTING_ASSET = "berlin-routing.osm.pbf"
    private const val TILES_ASSET = "berlin.pmtiles"
    private const val GLYPHS_ASSET = "glyphs"

    fun ensure(context: Context): Provisioned {
        val bundled = context.assets.list(ASSET_DIR)?.toSet() ?: emptySet()
        val outDir = File(context.filesDir, "map").apply { mkdirs() }
        return Provisioned(
            routingPbf = copyIfBundled(context, bundled, ROUTING_ASSET, outDir),
            pmtiles = copyIfBundled(context, bundled, TILES_ASSET, outDir),
            glyphsDir = if (GLYPHS_ASSET in bundled) {
                copyTree(context, "$ASSET_DIR/$GLYPHS_ASSET", File(outDir, GLYPHS_ASSET))
            } else {
                null
            },
        )
    }

    /**
     * Recursively copy an asset subtree to [dest]. Glyphs are hundreds of
     * small .pbf files, not one big one, so this walks the tree; the count is
     * used as a cheap "already unpacked" check to avoid recopying every launch.
     */
    private fun copyTree(context: Context, assetPath: String, dest: File): File? {
        val children = runCatching { context.assets.list(assetPath) }.getOrNull()
        if (children.isNullOrEmpty()) {
            // A leaf: copy the file itself.
            return runCatching {
                context.assets.open(assetPath).use { input ->
                    dest.outputStream().use { input.copyTo(it) }
                }
                dest
            }.getOrNull()
        }
        dest.mkdirs()
        for (child in children) {
            copyTree(context, "$assetPath/$child", File(dest, child))
        }
        return dest
    }

    private fun copyIfBundled(
        context: Context,
        bundled: Set<String>,
        name: String,
        outDir: File,
    ): File? {
        if (name !in bundled) return null
        val out = File(outDir, name)

        // Size of the packaged asset, used to detect a partial or stale copy.
        // openFd only works for uncompressed entries (see the noCompress block
        // in build.gradle.kts); if it ever fails, re-copy rather than trust a
        // file we can't verify.
        val assetSize = runCatching {
            context.assets.openFd("$ASSET_DIR/$name").use { it.length }
        }.getOrNull()

        if (assetSize == null || out.length() != assetSize) {
            // Copy to a temporary file first: an interrupted copy must not
            // leave a truncated file that looks complete on the next launch.
            val tmp = File(outDir, "$name.part")
            context.assets.open("$ASSET_DIR/$name").use { input ->
                tmp.outputStream().use { input.copyTo(it) }
            }
            if (!tmp.renameTo(out)) {
                tmp.delete()
                return null
            }
        }
        return out
    }

    /** The MapLibre style JSON, pointing at the local tiles and glyphs. */
    fun styleJson(context: Context, provisioned: Provisioned): String {
        val pmtiles = provisioned.pmtiles
        if (pmtiles == null) {
            // No basemap bundled: bare dark canvas; cameras/route still render.
            return """{"version":8,"name":"fallback","sources":{},"layers":[
                {"id":"background","type":"background",
                 "paint":{"background-color":"#10141a"}}]}"""
        }
        val template = context.assets.open("style_template.json")
            .bufferedReader().use { it.readText() }
        // MapLibre substitutes {fontstack}/{range} into the glyphs URL.
        val glyphsUrl = provisioned.glyphsDir?.let {
            "file://${it.absolutePath}/{fontstack}/{range}.pbf"
        } ?: ""
        return template
            .replace("__PMTILES_URL__", "file://${pmtiles.absolutePath}")
            .replace("__GLYPHS_URL__", glyphsUrl)
    }
}
