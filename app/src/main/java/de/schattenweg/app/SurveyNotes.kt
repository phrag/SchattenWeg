package de.schattenweg.app

import java.util.Locale

/**
 * A camera the user saw on the street and noted down. Private by construction:
 * kept in the app's own storage, drawn as an overlay, never sent anywhere (the
 * app has no INTERNET permission) and never fed to the router. The only way out
 * is a file the user chooses to export (see [SurveyNotes.toOsm]).
 *
 * There is deliberately no free-text field and no timestamp: a note records
 * where a lens is, not who was there or when.
 *
 * Pure Kotlin (no Android types) so it can be exercised on a plain JVM.
 */
data class SurveyNote(
    val id: Long,
    val lat: Double,
    val lon: Double,
    val kind: Kind,
    /** Bearing the lens faces, 0 = N, 90 = E; null when unknown or not fixed. */
    val directionDeg: Int?,
    val mount: Mount?,
) {
    /** OSM `camera:type` values; [UNKNOWN] leaves the tag off. */
    enum class Kind(val label: String, val osmValue: String?) {
        FIXED("Fixed", "fixed"),
        DOME("Dome", "dome"),
        PANNING("Panning", "panning"),
        UNKNOWN("Not sure", null),
    }

    /** OSM `camera:mount` values. */
    enum class Mount(val label: String, val osmValue: String) {
        WALL("Wall", "wall"),
        POLE("Pole", "pole"),
        CEILING("Ceiling", "ceiling"),
    }
}

object SurveyNotes {
    private const val HEADER = "schattenweg-survey-v1"

    /** One note per line, tab-separated. Plain text keeps it inspectable. */
    fun encode(notes: List<SurveyNote>): String =
        buildString {
            appendLine(HEADER)
            for (n in notes) {
                appendLine(
                    listOf(
                        n.id,
                        coord(n.lat),
                        coord(n.lon),
                        n.kind.name,
                        n.directionDeg ?: "",
                        n.mount?.name ?: "",
                    ).joinToString("\t"),
                )
            }
        }

    /**
     * Inverse of [encode]. A wrong header yields nothing and a malformed line
     * is skipped, so a damaged file loses a note rather than the whole set.
     */
    fun decode(text: String): List<SurveyNote> {
        val lines = text.lines()
        if (lines.firstOrNull()?.trim() != HEADER) return emptyList()
        return lines.drop(1).mapNotNull { parseLine(it) }
    }

    private fun parseLine(line: String): SurveyNote? {
        if (line.isBlank()) return null
        val f = line.split('\t')
        if (f.size != 6) return null
        val id = f[0].toLongOrNull() ?: return null
        val lat = f[1].toDoubleOrNull()?.takeIf { it in -90.0..90.0 } ?: return null
        val lon = f[2].toDoubleOrNull()?.takeIf { it in -180.0..180.0 } ?: return null
        val kind = SurveyNote.Kind.entries.firstOrNull { it.name == f[3] } ?: return null
        val dir = if (f[4].isEmpty()) {
            null
        } else {
            f[4].toIntOrNull()?.let(::normalise) ?: return null
        }
        val mount = if (f[5].isEmpty()) {
            null
        } else {
            SurveyNote.Mount.entries.firstOrNull { it.name == f[5] } ?: return null
        }
        return SurveyNote(id, lat, lon, kind, dir, mount)
    }

    /** A fresh id that cannot collide with any note already held. */
    fun nextId(notes: List<SurveyNote>): Long = (notes.maxOfOrNull { it.id } ?: 0L) + 1L

    /** Bearing folded into 0..359. */
    fun normalise(deg: Int): Int = ((deg % 360) + 360) % 360

    /**
     * An OSM XML file of new nodes, for opening in JOSM or Vespucci and
     * uploading from there under whichever account the user chooses.
     *
     * Negative ids mark the nodes as new, which is what editors expect of
     * not-yet-uploaded objects. Only tags the user actually gave are written:
     * nothing is guessed. `surveillance=*` (public / outdoor / indoor) is left
     * for the mapper to add in the editor, since it depends on what the lens
     * is looking at and the app cannot know. The direction is written only for
     * a fixed camera; for anything else a bearing means little.
     */
    fun toOsm(notes: List<SurveyNote>): String =
        buildString {
            appendLine("<?xml version='1.0' encoding='UTF-8'?>")
            appendLine("<osm version=\"0.6\" generator=\"Schattenweg\">")
            for ((i, n) in notes.withIndex()) {
                appendLine(
                    "  <node id=\"${-(i + 1)}\" action=\"modify\" visible=\"true\" " +
                        "lat=\"${coord(n.lat)}\" lon=\"${coord(n.lon)}\">",
                )
                appendLine(tag("man_made", "surveillance"))
                appendLine(tag("surveillance:type", "camera"))
                n.kind.osmValue?.let { appendLine(tag("camera:type", it)) }
                n.mount?.let { appendLine(tag("camera:mount", it.osmValue)) }
                if (n.kind == SurveyNote.Kind.FIXED && n.directionDeg != null) {
                    appendLine(tag("camera:direction", n.directionDeg.toString()))
                }
                appendLine("  </node>")
            }
            appendLine("</osm>")
        }

    /** Every value written is an enum constant or an int, so nothing to escape. */
    private fun tag(k: String, v: String) = "    <tag k=\"$k\" v=\"$v\"/>"

    /** Seven decimals is about a centimetre; Locale.ROOT pins the decimal point. */
    private fun coord(v: Double) = String.format(Locale.ROOT, "%.7f", v)
}
