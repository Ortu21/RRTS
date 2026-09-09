# ARCHITECTURE — RRTS

> Per AI e nuovi contributor: leggi questo prima di toccare `src/`.
> Dettagli gameplay in `README.md`, vincoli prodotto in `PRODUCT.md` + `DESIGN.md`,
> strategia AI in `docs/ai-knowledge.md`.

## Libreria vs binario

- `src/lib.rs` = tutti i domini come `pub mod`. Importa da `rust_rts::...` in test/tool esterni.
- `src/main.rs` = solo CLI + assemblaggio `App`. Niente sistemi qui.
- Compat: `rust_rts::view` è alias di `rust_rts::session` (rinominato, vedi sotto).

## Mappa moduli (cosa possiede cosa)

| Modulo | Possiede | Non toccare da qui |
|---|---|---|
| `scenario` | `Scenario` (Playground/Benchmark/Skirmish), centri e mete | sistemi, spawn |
| `session` (ex `view`) | `SessionControl` (chi comanda), `ViewState` (cosa guardi) | telecamera (`camera`), fog dati |
| `world` | terreno procedurale seedato, ostacoli | unità, ordini |
| `camera` | `RtsCamera`, pan/zoom/rotate, `CameraFocus` | ordini, visibilità |
| `picking` | `ground_position`, `ray_box_distance` puri | stato, fog-gating (ai chiamanti) |
| `selection` | `Selected`, drag/box, `can_command` gate | ordini, ispezione neutra ≠ comando |
| `orders` | `UnitOrder` intento + `UnitOrderQueue`, `PendingOrder` targeting | locomozione, danni |
| `orders::lines` | grafica ordini BAR-style | decisioni |
| `movement` | consuma `Route`, separazione soft, `MoveTarget` | planning, targeting |
| `navigation` | `NavGrid` 240×240, Theta*+LOS, `PATHS_PER_FRAME=128`, clearance scafo | slot formazioni (in `formation`) |
| `formation` | `formation_slots`, `skirmish_slots` puri | query Bevy |
| `spatial` | hash uniforme/frame per avoidance+targeting | decisioni |
| `combat` | HP/armi/proiettili/morti, `AttackTarget`, `Chasing`/`HoldFire` | intento (in `orders`) |
| `economy` (+`balance`) | `Economy`, costi/redditi, `BuildingKind` stats | spawn edifici (in `structures`) |
| `structures` | `Building`, `Construction`, `placement_rule`, work trickle | code produzione |
| `production` | `Factory` code (`MAX_QUEUE=12`), rally, blocked-retry | economia |
| `fog` | `VisibilityMap`, shroud+fog 4Hz, `can_target` gate | rendering overlay (stesso plugin, solo player team) |
| `units` (+`archetype`) | spawn, `UnitKind` stats pure | ordini/movimento/combat |
| `game_over` | `MatchResult`, banner, `R` restart | AI stop (l'AI legge `MatchResult`) |
| `ai` | `AiConfig`/`AiSnapshots`/`AiState`, `ai_tick` 1Hz + `micro_tick` 4Hz | query nemiche dirette (solo snapshot!) |
| `ai::{strategy,combat,scout,opponent,threat,utility,memory,snapshot,executor,planner,director}` | decisione pura/data-driven | attuatori (in `executor` via stessi `queue_*` del player) |
| `ui::shell` | layout top-bar/dock/sidebar, azioni sessione | sim |
| `ui::industry` | construction/production UI (presentazione) | decisioni (in sim) |
| `ui::debug` | overlay opt-in, `DebugSettings` | sim (l'AI non legge mai da qui) |
| `benchmark::{cli,report,profile}` | workload, misura, storia | gameplay |
| `replay` | `TickChecksums` + `fnv1a_64`, futuri input-log | gameplay |

## Flussi

```
input → picking → selection → orders → navigation → movement
                                      ↘ combat ↩ spatial
economy → structures → production → units → fog → game_over
ai: snapshot(4Hz) → strategy(1Hz) → executor → stessi attuatori player
                       ↘ micro(4Hz, budget 2)
ui: legge tutto, scrive solo via stessi canali player (mai mutazione diretta sim)
```

Order schedule: `(snapshot, ai_tick, micro_tick).chain().after(MovementSystems).after(FogSystems)` + `run_if(ai_active)`. Vedi `ai/mod.rs`. Non riordinare senza aggiornare `ai_active` + `MatchResult` gate.

## Regole dure (da `docs/ai-knowledge.md`)

1. Determinismo bit-identico: `BTreeMap`, sort per `to_bits`/`total_cmp`, tick fisso, mai `HashMap/rand/time` in sim.
2. Fog onesto: AI solo via `AiSnapshot + EnemyMemory`. Mai query nemiche dirette. Gate `fog-honest`.
3. Max ~16 param Bevy: funzioni pure fuori dai system, `ParamSet` se crescono. `#[allow(too_many_arguments)]` = segnale di split, non soluzione.
4. Data-driven: niente `if kind == HeavyTank` nel core AI; tabelle + `personalities/*.ron` + pesi `Personality`.
5. Misura: headless = CPU sim, grafico = +rendering, trace = diagnostica. Mai mixare per confronti.
6. Test gate ogni fase: `cargo test + ai-test + ai-suite quick + ai-scenarios` + delta checksum/winrate dichiarato prima.

## File grandi — piano split (non in questo PR)

- `ai/strategy.rs ~3k`: → `strategy/{decide,waves,micro,spots,personality}.rs` con `mod.rs` che re-esporta. Primo candidato: `wall_slots/find_*_spot` → `spots.rs`.
- `combat/mod.rs ~2.3k`: → `combat/{acquire,fire,projectile,death,guard}.rs`.
- `navigation/mod.rs ~1.6k`: → `navigation/{grid,theta,congestion,budget}.rs`.
- `ui/industry.rs ~1k`, `ui/shell.rs ~0.9k`: già separati per ruolo; non unire, semmai estrarre `placement_preview.rs` da industry.

Regola: split solo con re-export invariato + `cargo test` verde. Niente rename pubblici senza alias deprecato (come `view`→`session`).

## Come aggiungere cose

- Nuova unità/edificio: riga in `archetype`/`balance` + pesi `Personality` + `.ron`, mai `if` nel core.
- Nuovo ordine: variante `UnitOrder` + consumo in `movement` + grafica in `lines` + gate fog/comando.
- Nuovo workload: variante `Workload` + orchestrazione in `benchmark`, riusa plugin gioco.
- Nuovo tool debug: variante `DebugTool` in `ui::debug`, default OFF, mai letto dalla sim.

## Vault esterno

`docs/ai-knowledge.md` linka al vault Obsidian `../RRTS-AI-Vault` (non in git) per *perché/scartati*.
Tutto il *cosa fare* (Obiettivo/Assert/Metriche/NON-fare) resta in `docs/` e qui. Se manca il vault, `https://www.gameaipro.com/` + questo file bastano per lavorare.
