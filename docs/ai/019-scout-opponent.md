# 0.0.19 — Scouting frontiera + opponent modeling (spec snella)

Madre: `docs/ai-advanced-plan.md` §3 (0.0.19). Studio: vault `50-Scouting-Opponent/`.
Fonti: Welsh Pro2 Ch27, Walsh Pro2 Ch28, Zielinski Pro1 Ch33.

## Obiettivo

`scout-blitz explored@120s` >12%, first-blood anticipato, `classify` ≥80% in test,
`scout-nav-clean` verde. Risolve diagnosi nota: turtle cieco + ramo `Scout` morto
(`army_count==0` impossibile col Commander vivo).

## Cosa fare

- Nuovo `src/ai/scout.rs`: `frontier_target()` su celle `explored==false`,
  `novelty − minaccia×peso` (pesi in `Personality`), max 2 scout, waypoint threat-aware.
- Nuovo `src/ai/opponent.rs`: `classify()` (conteggi freschi + timing primo contatto +
  edifici) → shift mix/courage ±0.1 da tabella.
- `src/ai/memory.rs`: `Contact.confidence=1/(1+age/60)`, edifici ricordati usati,
  `attack_destination()` su base ricordata. Riesuma `visible_enemy_buildings`.
  Intento `Scout` svincolato dal conteggio armata.

## Assert / metriche

`frontier_prefers_unseen`, `scout_avoids_hotspot`, `building_memory_redirects_attack`,
`classify_rusher`. Metriche: explored, first-blood, `median_explored_pct`.

## NON fare

Switch totale strategia mid-game, ML, scritture su `VisibilityMap`.

## Delta atteso

Checksum scout/attacco cambiano strutturalmente (fix voluto del rosso pre-esistente).
Dichiarare prima.
