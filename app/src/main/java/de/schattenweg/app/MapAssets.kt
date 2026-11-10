package de.schattenweg.app

import android.content.Context
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.time.LocalDate

/**
 * Provisions the bundled map data into app-private storage.
 *
 * Both the routing data (read by the Rust core) and the PMTiles basemap
 * (read by MapLibre, which needs byte-range access that Android assets can't
 * provide) are copied from APK assets to `filesDir/map/` on first launch.
 *
 * The assets are produced by `scripts/build_map_assets.sh`; a build without
 * them still runs (routes unavailable / plain background) so CI can assemble
 * the APK without the ~100 MB of Berlin data.
 */
object MapAssets {

    data class Provisioned(
        /**
         * The OSM extract the graph is scored from, or null. A normal build
         * does not bundle it: the cache below *is* the routing data and the
         * 59 MB extract stayed out of the APK. Only a build whose cache could
         * not be pre-built ships it, so the app can score the graph itself.
         */
        val routingPbf: File?,
        /**
         * The exposure-scored graph. Normally the bundled cache, opened on its
         * own. With only an extract bundled, the path where the core writes its
         * own cache on the first launch (so later launches skip the several-
         * second scoring pass). Null when neither is bundled.
         */
        val routingCache: File?,
        /** Offline basemap tiles, or null if not bundled. */
        val pmtiles: File?,
        /** Directory holding the label glyph tree, or null if not bundled. */
        val glyphsDir: File?,
    )

    private const val ASSET_DIR = "map"
    private const val ROUTING_ASSET = "berlin-routing.osm.pbf"

    /** Core-written cache of the scored graph; see [Provisioned.routingCache]. */
    private const val ROUTING_CACHE = "berlin-routing.graphcache"
    private const val TILES_ASSET = "berlin.pmtiles"

    /** Written by build_map_assets.sh: `osm_snapshot=` and `built_utc=` lines. */
    private const val DATA_INFO_ASSET = "data-info.txt"
    private const val GLYPHS_ASSET = "glyphs"

    /**
     * Copying the bundled data out failed, almost always for lack of storage
     * (the cache and tiles are ~200 MB together). [message] is user-facing.
     */
    class ProvisionException(message: String, cause: Throwable? = null) :
        IOException(message, cause)

    private const val TAG = "SchattenWeg"

    /** Headroom kept free beyond the file itself, so the phone isn't left at 0 bytes. */
    private const val FREE_SPACE_MARGIN = 32L * 1024 * 1024

    /**
     * @throws ProvisionException when a file cannot be copied out (typically a
     *   full disk). Partial copies are removed, so the next launch starts clean.
     */
    fun ensure(context: Context): Provisioned {
        val bundled = context.assets.list(ASSET_DIR)?.toSet() ?: emptySet()
        val outDir = File(context.filesDir, "map").apply { mkdirs() }
        // A copy killed mid-way (process death, full disk) leaves a .part
        // behind; it is never read, so reclaim the space before copying again.
        outDir.listFiles { f -> f.name.endsWith(".part") }?.forEach { it.deleteRecursively() }
        val install = installStamp(context)
        val routingPbf = copyIfBundled(context, bundled, ROUTING_ASSET, outDir)
        if (routingPbf == null) {
            // Installs before the extract left the APK extracted it to filesDir.
            // It is no longer read, so give the space back (59 MB).
            File(outDir, ROUTING_ASSET).delete()
        }
        return Provisioned(
            routingPbf = routingPbf,
            // The cache pre-built by build_map_assets.sh. With only an extract
            // bundled the path is still returned, so the core can write its own.
            routingCache = copyIfBundled(context, bundled, ROUTING_CACHE, outDir)
                ?: routingPbf?.let { File(outDir, ROUTING_CACHE) },
            pmtiles = copyIfBundled(context, bundled, TILES_ASSET, outDir),
            glyphsDir = if (GLYPHS_ASSET in bundled) {
                provisionGlyphs(context, outDir, install)
            } else {
                null
            },
        )
    }

    /**
     * Unpack the label glyphs (hundreds of small .pbf files) once per install.
     *
     * Same stamp rule as the compressed graph cache: the tree is copied into a
     * `.part` directory, renamed into place, and only then stamped, so a copy
     * that didn't finish is never mistaken for a complete one. Labels are not
     * essential, so a failure returns null (the style then drops its label
     * layers) instead of stopping the app.
     */
    private fun provisionGlyphs(context: Context, outDir: File, install: String?): File? {
        val dest = File(outDir, GLYPHS_ASSET)
        val stamp = File(outDir, "$GLYPHS_ASSET.stamp")
        if (install != null && dest.isDirectory &&
            runCatching { stamp.readText() }.getOrNull() == install
        ) {
            return dest
        }
        stamp.delete()
        val tmp = File(outDir, "$GLYPHS_ASSET.part")
        return try {
            tmp.deleteRecursively()
            copyTree(context, "$ASSET_DIR/$GLYPHS_ASSET", tmp)
            dest.deleteRecursively()
            if (!tmp.renameTo(dest)) throw IOException("could not move glyphs into place")
            if (install != null) runCatching { stamp.writeText(install) }
            dest
        } catch (e: IOException) {
            Log.w(TAG, "label glyphs not unpacked; map labels disabled", e)
            tmp.deleteRecursively()
            null
        }
    }

    /** Recursively copy an asset subtree to [dest]; throws on any failed file. */
    private fun copyTree(context: Context, assetPath: String, dest: File) {
        val children = context.assets.list(assetPath)
        if (children.isNullOrEmpty()) {
            context.assets.open(assetPath).use { input ->
                dest.outputStream().use { input.copyTo(it) }
            }
            return
        }
        dest.mkdirs()
        for (child in children) {
            copyTree(context, "$assetPath/$child", File(dest, child))
        }
    }

    private fun copyIfBundled(
        context: Context,
        bundled: Set<String>,
        name: String,
        outDir: File,
    ): File? {
        if (name !in bundled) return null
        val out = File(outDir, name)

        // Is the copy in filesDir current? Two ways to tell.
        //  * Stored (uncompressed) entries: openFd gives the packaged size, and
        //    a copy of any other size is partial or stale.
        //  * Compressed entries (the graph cache; see the noCompress block in
        //    build.gradle.kts): openFd refuses them, and the size can't be read
        //    without inflating, so a stamp file beside the copy records which
        //    install it came from. Every install or update changes
        //    lastUpdateTime, so a new APK always re-extracts, and an unchanged
        //    one never does. Without this, a 110 MB file would be re-copied on
        //    every launch.
        val assetSize = runCatching {
            context.assets.openFd("$ASSET_DIR/$name").use { it.length }
        }.getOrNull()
        val stamp = File(outDir, "$name.stamp")
        val install = installStamp(context)

        val upToDate = if (assetSize != null) {
            out.length() == assetSize
        } else {
            install != null && out.exists() &&
                runCatching { stamp.readText() }.getOrNull() == install
        }

        if (!upToDate) {
            // Copy to a temporary file first: an interrupted copy must not
            // leave a truncated file that looks complete on the next launch.
            stamp.delete()
            // The stale copy is about to be replaced; count its space as free.
            val reclaimable = if (out.exists()) out.length() else 0L
            if (assetSize != null &&
                outDir.usableSpace + reclaimable < assetSize + FREE_SPACE_MARGIN
            ) {
                throw ProvisionException(notEnoughSpace(assetSize))
            }
            val tmp = File(outDir, "$name.part")
            try {
                context.assets.open("$ASSET_DIR/$name").use { input ->
                    tmp.outputStream().use { input.copyTo(it) }
                }
            } catch (e: IOException) {
                tmp.delete()
                // A compressed entry's size is unknown up front, so a full
                // disk shows up here instead of in the check above.
                throw ProvisionException(notEnoughSpace(assetSize), e)
            }
            if (!tmp.renameTo(out)) {
                tmp.delete()
                return null
            }
            // Written last, so a copy that didn't finish is never stamped.
            if (assetSize == null && install != null) {
                runCatching { stamp.writeText(install) }
            }
        }
        return out
    }

    /**
     * The day the bundled OpenStreetMap extract was cut (its replication
     * timestamp, not the day it was downloaded), i.e. how fresh the cameras
     * are. Null when the build bundles no data, or the date is missing or
     * "unknown", so the UI simply shows nothing rather than a wrong date.
     */
    fun snapshotDate(context: Context): LocalDate? = runCatching {
        context.assets.open("$ASSET_DIR/$DATA_INFO_ASSET").bufferedReader().useLines { lines ->
            lines.map { it.trim() }
                .firstOrNull { it.startsWith("osm_snapshot=") }
                ?.removePrefix("osm_snapshot=")
                // "2026-10-09T20:21:04Z" -> the date part.
                ?.take(10)
                ?.let { LocalDate.parse(it) }
        }
    }.getOrNull()

    private fun notEnoughSpace(bytes: Long?): String {
        val need = bytes?.let { " (about ${(it + FREE_SPACE_MARGIN) / (1024 * 1024)} MB)" } ?: ""
        return "Not enough free storage to unpack the Berlin map data$need. " +
            "Free up some space and reopen the app."
    }

    /** Changes whenever the app is installed or updated; null if unreadable. */
    private fun installStamp(context: Context): String? = runCatching {
        context.packageManager
            .getPackageInfo(context.packageName, 0)
            .lastUpdateTime
            .toString()
    }.getOrNull()

    /** The MapLibre style JSON, pointing at the local tiles and glyphs. */
    fun styleJson(context: Context, provisioned: Provisioned): String {
        val pmtiles = provisioned.pmtiles
        if (pmtiles == null) {
            // No basemap bundled: bare dark canvas; cameras/route still render.
            return """{"version":8,"name":"fallback","sources":{},"layers":[
                {"id":"background","type":"background",
                 "paint":{"background-color":"#171c26"}}]}"""
        }

        val template = context.assets.open("style_template.json")
            .bufferedReader().use { it.readText() }
        val withTiles = template.replace("__PMTILES_URL__", "file://${pmtiles.absolutePath}")

        val glyphs = provisioned.glyphsDir
        if (glyphs != null) {
            // MapLibre substitutes {fontstack}/{range} into the glyphs URL.
            return withTiles.replace(
                "__GLYPHS_URL__",
                "file://${glyphs.absolutePath}/{fontstack}/{range}.pbf",
            )
        }

        // No glyphs bundled (assets not rebuilt): MapLibre Native fails the
        // whole style if a symbol layer has no resolvable glyphs URL, which
        // would blank the basemap entirely -- roads and water included. So
        // drop the glyphs key and every label (symbol) layer, leaving a
        // fully renderable basemap with no text until the fonts are built.
        val root = JSONObject(withTiles)
        root.remove("glyphs")
        val layers = root.getJSONArray("layers")
        val kept = org.json.JSONArray()
        for (i in 0 until layers.length()) {
            val layer = layers.getJSONObject(i)
            if (layer.optString("type") != "symbol") kept.put(layer)
        }
        root.put("layers", kept)
        return root.toString()
    }
}
