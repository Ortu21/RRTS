# 0.0.18 — Macro planner + difesa posizionale (spec snella)

Madre: `docs/ai-advanced-plan.md` §3 (0.0.18). Studio: vault `40-Macro-Planner/`,
`20-Threat-Influence/`. Fonti: Lewis Pro3 Ch13, Mark Pro2 Ch30, Zielinski Pro3 Ch24,
Foged/Horswill Pro2 Ch26, Johnson Pro3 Ch26.

## Obiettivo

`eco-race income@180s` +15%, `hold-the-base hp` >55%, `nav-zero-failures` verde.

## Cosa fare

- Nuovo `src/ai/planner.rs`: `plan_build()` con riserva `stock-reserved>cost` +
  `time_to_afford`, orizzonte 60s, costi reali `UNIT_COSTS+work/power`. LabT2 su
  planner+income, non solo `tick>=360`.
- Torrette ad anello verso `threat.hotspot()`, primi muri davanti a torretta
  (`wall_slots()` validata da `valid_ground+spawn_ok` + nav-reachable). Rally per tipo
  da tabella offset. Threat layer `DPS×HP×range+torrette`, peso memoria `1/(1+age/60)`.

## Assert / metriche

`planner_waits_when_broke`, `labt2_needs_eco`, `wall_slots_walkable`,
`turret_faces_threat`. Metriche: income, base_hp, `median_match_ticks` non esplode.

## NON fare

Multi-base, terza factory, micro nuovo, kiting, scouting nuovo.

## Delta atteso

Checksum cambiano su build order/posizioni. Dichiarare prima quali scenari migliorano.
