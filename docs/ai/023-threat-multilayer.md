# 0.0.23 — Threat multistrato range-aware + cache (spec snella)

Madre: `docs/ai-advanced-plan.md` §4 debito futuro (threat multilayer
range-aware Mark/Zielinski/Lewis). Studio: vault `20-Threat-Influence/`.
Fonti: Mark Pro2 Ch30 (modular: layer separati, query composte), Lewis Pro2
Ch29 (point-based influence, query veloci), Zielinski Pro3 Ch24 (being where
it counts: la gittata decide gli assedi).

## Obiettivo

La threat smette di essere `hp*dps` puntiforme: ogni sorgente proietta
`dps×hp` entro la sua gittata (da tabella, mai branch per-kind) con falloff
lineare, su 3 layer (`live`, `remembered`, `static_def`). Una `ThreatCache`
per-team (chiave = snapshot tick) riduce le build a 1 per team per
strategy-tick: `decide()`/`executor` ricevono la mappa, non la ricostruiscono
(oggi fino a 5 build per team per tick: 2 in `decide`, 3 in `executor`).

## Cosa fare

- `src/ai/threat.rs`: `ThreatMap` += layer `live/remembered/static_def`
  (`cells` resta la somma = API combinata invariata), `deposit()` con spread
  `w*(1-dist/(range+side))` in ordine row-major, `threat_range()` da tabella
  (max primaria/secondaria, 0 disarmati), `building_threat` invariato nei
  valori (spread con gittata torretta 24/28), muri restano 0 (nessun DPS: lo
  channeling è del nav). `query()`/`hotspot()`/`mean()`/`max()` invariati
  (leggono il combinato). Nuovi: `query_static()`, `means()`,
  `ThreatCache` (`get_or_build`/`get`, `BTreeMap`, tick-keyed).
- `src/ai/strategy.rs`: `decide_with_threat(..., threat)` +
  `base_under_threat_with_map(...)`; `decide()`/`base_under_threat()` restano
  wrapper (build + delega: tutti i test esistenti passano invariati).
- `src/ai/executor.rs`: nuovo param `cached_threat: Option<&ThreatMap>`
  (lazy build locale se `None` = stesso comportamento di prima).
- `src/ai/mod.rs`: `ThreatCache` resource; `ai_tick` 1 build per team per tick
  e passa la mappa a `decide_with_threat` + executor; `micro_tick` riusa la
  cache senza ricostruire (`get`, mai `get_or_build`: il micro non ordina mai
  Build/Scout).
- `src/ai/director.rs`: `threat-sane` += medie layer nel detail.

## Assert / metriche

`spread_falls_off_with_distance`, `spread_touches_bounded_cells`,
`layers_sum_to_combined`, `cache_reuses_same_tick_and_rebuilds_on_new_tick`,
`decide_with_threat_matches_decide`, `ron` n/a. Metriche: `ai-test` e
`ai-scenarios` PASS deterministici; `hold-the-base hp` resta >55%.

## NON fare

Blur iterativo (uno step di spread basta: Mark single-pass), nuovi layer oltre
i 3, pesi per-kind nel core, hot-reload, TrueSkill, nuove dipendenze.

## Delta atteso (dichiarato prima)

I valori `query()`/`hotspot()` cambiano strutturalmente (spread in gittata):
checksum IDENTICI attesi solo negli scenari scriptati (`micro-duel`,
`siege`, `lance-hold`). Negli scenari AI-driven (`scout-blitz`, `eco-race`,
`hold-the-base`, `no-cheat`, `counter-comp`, `commander-snipe`) i checksum
POSSONO cambiare (rotte scout, anchor torrette/muri, richiami difensivi):
devono restare PASS + deterministici tra repeat. Direzione attesa: hotspot
verso le batterie a lunga gittata, scout che evitano gli inviluppi di tiro.
