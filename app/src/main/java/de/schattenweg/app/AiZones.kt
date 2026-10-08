package de.schattenweg.app

/**
 * Where Berlin authorities run, or have announced, AI behaviour detection on
 * CCTV ("KI-gestützter Videoschutz"). Display-only: these never feed the
 * router, because the exact camera positions are not public (the Senate
 * declined to release them) and a zone is a statement about policy, not a count
 * of lenses. Routing still scores only the cameras OSM has mapped.
 *
 * Geometry is deliberately crude -- a circle around a landmark. The authorities
 * publish no boundaries, only names; the real edge will be the signage the
 * Senate promised at every entrance to a protected area. Replace the circles
 * with surveyed outlines as that signage goes up.
 *
 * Sources: Berlin Abgeordnetenhaus Drucksache 19/26970 (Senate answer of
 * 6 Sep 2026) and the Polizei Berlin press release of 24 Sep 2026.
 */
internal data class AiZone(
    val id: String,
    val name: String,
    val lat: Double,
    val lon: Double,
    /** Illustrative radius, not an official boundary. */
    val radiusM: Double,
    val status: Status,
    /** What is known about timing, shown on the info card. */
    val note: String,
    /** What the system is said to detect there. */
    val detects: String,
) {
    enum class Status(val label: String) {
        /** Cameras are being installed or calibrated; no live alerts yet. */
        COMMISSIONING("Being set up"),

        /** Announced with a date, not yet started. */
        PLANNED("Planned"),
    }
}

/** When the table below was last checked against the sources. */
internal const val AI_ZONES_AS_OF = "October 2026"

private const val POLICE_DETECTS =
    "Violence (hitting, kicking) and people lying motionless. The screen stays " +
        "black until the software flags an event; an officer then decides."

private const val OBJEKTSCHUTZ_DETECTS =
    "Entering restricted areas, crossing virtual lines, climbing fences, " +
        "vandalism and abandoned objects."

internal val AI_ZONES: List<AiZone> = listOf(
    AiZone(
        id = "kotti",
        name = "Kottbusser Tor",
        lat = 52.4990,
        lon = 13.4178,
        radiusM = 150.0,
        status = AiZone.Status.COMMISSIONING,
        note = "About 30 cameras. Set-up and calibration began 24 Sep 2026, " +
            "roughly four weeks, then a test phase. Not yet issuing alerts.",
        detects = POLICE_DETECTS,
    ),
    AiZone(
        id = "warschauer",
        name = "Warschauer Brücke",
        lat = 52.5060,
        lon = 13.4498,
        radiusM = 150.0,
        status = AiZone.Status.PLANNED,
        note = "Planned for completion by the end of 2026. No firm date.",
        detects = POLICE_DETECTS,
    ),
    AiZone(
        id = "alex",
        name = "Alexanderplatz",
        lat = 52.5219,
        lon = 13.4132,
        radiusM = 250.0,
        status = AiZone.Status.PLANNED,
        note = "Planned for 2027.",
        detects = POLICE_DETECTS,
    ),
    AiZone(
        id = "goerli",
        name = "Görlitzer Park / Wrangelkiez",
        lat = 52.4966,
        lon = 13.4350,
        radiusM = 350.0,
        status = AiZone.Status.PLANNED,
        note = "Planned for 2027. Reported to cover the park's busy entrances " +
            "and crossings, not the whole park.",
        detects = POLICE_DETECTS,
    ),
    AiZone(
        id = "rathaus",
        name = "Rotes Rathaus",
        lat = 52.5186,
        lon = 13.4083,
        radiusM = 70.0,
        status = AiZone.Status.PLANNED,
        note = "Building-protection pilot. Test phase planned for Q2 2027.",
        detects = OBJEKTSCHUTZ_DETECTS,
    ),
    AiZone(
        id = "stadthaus",
        name = "Altes Stadthaus (Innenverwaltung)",
        lat = 52.5153,
        lon = 13.4154,
        radiusM = 70.0,
        status = AiZone.Status.PLANNED,
        note = "Building-protection pilot. Test phase planned for Q2 2027.",
        detects = OBJEKTSCHUTZ_DETECTS,
    ),
    AiZone(
        id = "juedmus",
        name = "Jüdisches Museum",
        lat = 52.5020,
        lon = 13.3954,
        radiusM = 70.0,
        status = AiZone.Status.PLANNED,
        note = "Building-protection pilot. Test phase planned for Q2 2027.",
        detects = OBJEKTSCHUTZ_DETECTS,
    ),
)
