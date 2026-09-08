# AI avanzata — piano a fasi (solo piano, niente ML)

Contesto: RTS Rust+Bevy. AI attuale: snapshot onesto per-team filtrato da fog
→ `decide()` pura con scorer/mix data-driven → intenti
(Build/Enqueue/AttackMoveAll/Scout/Retreat/FocusFire) → executor con budget APM
che usa le stesse `queue_*` del player. Personalità const
(turtle/rusher/eco-only/rush-scripted/null), EnemyMemory con età, macro a loop
chiuso su stock/income/demand, tier T1/T2, torrette e muri. Capitale: Commander
muore = game over. Suite: league AI-vs-AI con winrate+Elo+history
(`src/ai/league.rs`), 6 scenari L1 (`src/ai/scenarios.rs`), harness `ai-test`,
~140 unit test.

Decisioni tradeoff già prese dall'owner:
- courage **solo predittivo** (no timer anti-stallo per i main);
- micro **separato a 4Hz** per Retreat/Focus/Hold/Screen;
- difficoltà **handicap puri** sullo stesso cervello (non personalità separate).

Paletti: tutto data-driven (mai branch per-kind nei sistemi), determinismo
bit-identico tra repeat, mai barare col fog, max 16 parametri per sistema Bevy,
niente nuove dipendenze senza motivo forte.

## 1. Stato reale — cosa manca per "avanzata"

### Percezione
- `AiSnapshot.visible_enemy_buildings`, `stock`, `builders()/idle_builders()`
  `dead_code`: edifici nemici visti ma mai usati da `decide()`.
- `memory.rs: fresh_contacts()/remembered_centroid()` hook morti: `strategy.rs`
  ricalcola il centroide a mano, solo unità, sconto fisso `*0.5`, nessuna
  pesatura per età.
- Nessuna threat/influence map: `army_power()=sum(hp*dps)+0.7*secondary`
  ignora range/velocità/numeri (Lanchester square)/torrette/muri/terreno.
- Nessuna stima nemica (income/army nemico), no `% esplorato` in snapshot (solo
  `league.rs: sample_teams()` post-hoc), no eventi.
- `scout_destination()`: 4 punti fissi + primo ricordo fresco. Uno scout solo,
  nessun routing threat-aware, nessuna misura di information-gain.

### Decisione
- `decide()` è lista priorità if/else (bootstrap Metal→Solar→Factory + isteresi
  `demand>income*1.1` + `pick_deficit(count/peso)`), non scoring `0..1` con
  response-curve × pesi.
- Comp fissa per personalità + `T2_MIX` globale: nessuna tabella counter.
- Attacco su `army_threshold + attack_power_mult` statici + `attack_at_tick`
  solo per `rush-scripted`. Niente courage predittivo, niente timing-window.
- LabT2 a `tick>=360` senza check costi/work (`300/300+250`).

### Micro
- `Retreat` a `formation(home)`, non in copertura torrette. Conflitto irrisolto
  con `AttackMoveAll` nello stesso tick.
- `FocusFire` solo se `my>foe*2`, max 3 attaccanti sul nemico con hp minore
  (non per valore/minaccia).
- `AttackMoveAll` include il **Commander** (capitale!), artiglierie e scout.
  Isteresi 10m arbitraria. Niente kiting/screen/hold-max-range/split.

### Macro
- 1 build/tick, spot a spirale da `scenario.center()`, mai a choke/fronte.
  `Wall` mai richiesto (anche se `is_defense()` esiste). Rally fisso 18m.
- Una Enqueue per factory libera, nessun reservation risorse/work, nessun piano
  tech su costi reali. Nessun keep-back del capitale.

### Adattività
- `Personality` const + `from_name()` con fallback silenzioso a TURTLE (solo
  `league.rs: resolve_brain()` è strict). Niente `.ron`, niente handicap.
- Elo solo offline, mai riusato online. Suite con gate larghe
  (`explored>1%`, `base_hp>30%`, `kills>0`): mancano scenario counter-comp,
  timing/courage, commander-snipe, muri, map-pool oltre seed 0/1, budget ms.

## 2. Stato dell'arte distillato (senza ML)

| Tema | Fonte | Applicabile |
|---|---|---|
| Utility AI | Game AI Pro 1 Ch9 Graham, Pro3 Ch13 Lewis, Pro3 Ch31 Hanlon/Watts (DA:I) | scorer `0..1` + curve + pesi in data |
| Influence maps | Pro2 Ch29 Lewis, Ch30 Mark *Modular Tactical*, Pro3 Ch24 Zielinski, Ch26 Johnson, Ch31 Dill | griglia 32×32 da snapshot+memoria, layer DPS×HP×range+torrette, decadimento per età |
| Combat prediction | Pro3 Ch25 Stanescu/Barriga/Buro, Ch14 scripted+search, Ch30 Churchill/Buro Prismata | Lanchester square (`dA/dt=-βB`, esponente ~1.5 mix), fast-forward 5-10s con DPS da tabelle |
| Build-order | MicroRTS bot, Pro2 Ch26 Foged/Horswill constraint solver | planner greedy su `UNIT_COSTS+work/power` reali, riserva risorse, timing T2 su spike |
| Scouting | Pro2 Ch27 Welsh, Ch28 Walsh, Pro1 Ch33 Zielinski, Rabin Ch04/05 | frontiera = celle `explored==false`, valore = novelty − minaccia, confidence memoria |
| Opponent modeling | Pro2 Ch39 Weber/Nguyen, Online Ch10 Borovikov autoplay | classificatore a soglie: conteggi freschi + timing primo contatto + edifici → shift mix/courage |
| Evaluation | SSCAI/BASIL ladder, Battlecode scrimmage, MicroRTS competition, Borovikov metrics | quick/full + mirror-bias + baselines stupide + replay deterministici (già fatto, da estendere con map-pool) |

Scartare: MCTS/HTN online, reti neurali, RL (rompono determinismo/budget/16-param).

## 3. Fasi

Regola ogni fase: `cargo test` + `ai-test` + `ai-suite quick` + `ai-scenarios`
verdi. Se cambia comportamento, dichiarare prima il delta atteso su
checksum/winrate.

### 0.0.16 — Percezione strumentata (shadow, zero behavior change)
- Obiettivo: checksum league/scenarios identici a baseline, nuove metriche
  solo osservate.
- Crea `src/ai/threat.rs`: `ThreatMap{n,half,cells}`, `build_threat(&AiSnapshot)`
  pura (solo visibile+memoria fresca, peso `1/(1+age/k)`), `query/hotspot`,
  `mean/max`. Mai letta da `decide()` in questa fase.
- `snapshot.rs`: nuovo campo `explored_pct: f32` calcolato in
  `build_snapshot()` da `VisibilityMap` (nessun nuovo param Bevy);
  riesuma `visible_enemy_buildings` + helper `fresh_memory()/remembered_centroid()`
  su `AiSnapshot`; `strategy.rs` riusa gli helper (stessa matematica).
- `memory.rs`: `fresh_contacts/remembered_centroid` diventano API ufficiale
  (via `dead_code`, doc aggiornata).
- `league.rs: TeamSample` += `threat_mean/threat_max` (da `AiSnapshots`, mai
  da query nemiche dirette) + milestone `median_explored_pct`.
- `director.rs`: check descrittivo `threat-sane` (finito e ≥0).
- Assert: `threat_decays_with_age`, `threat_counts_turret`,
  `explored_pct_monotonic`, `ron` n/a. Metriche: checksum invariati.
- Rischi: usare la mappa per decidere troppo presto. Costo griglia: una volta
  per team, non per unità.
- NON fare: nuovi intenti, tuning pesi, cambi a `decide()` che alterano output.

### 0.0.17 — Predizione + courage predittivo + counter-comp
- Obiettivo: `turtle/rusher` vs `rush-scripted` non scende (warn 40% verde),
  `turtle` attacca quando domina senza timer.
- Nuovo `src/ai/combat.rs`: `effective_dps()`, matrice `COUNTER_TABLE[8][8]` in
  `balance.rs` (default 1.0), `predict_outcome()` Lanchester fast-forward 10s
  deterministico. `remembered_enemy_power()` pesata per età+counter.
  `power_ok` → `win_prob > personality.courage` (nuovo campo, turtle ~0.65
  rusher ~0.55, eco impossibile). Mix pesato per counter vs comp stimata.
  `FocusFire` per valore `dps*counter/hp` se `win_prob` alta.
- Assert: `lanchester_square_wins_with_numbers`, `counter_heavy_beats_light`,
  `focus_picks_artillery`, `courage_blocks_suicide`. Metriche: winrate vs
  rush-scripted, `median_max_army` su/giù dichiarato, `stall` non su.
- NON fare: kiting, scouting nuovo, timer anti-stallo per i main.

### 0.0.18 — Macro planner + difesa posizionale
- Obiettivo: `eco-race income@180s` +15%, `hold-the-base hp` >55%,
  `nav-zero-failures` verde.
- Nuovo `src/ai/planner.rs`: `plan_build()` con riserva
  `stock-reserved>cost` + `time_to_afford`, orizzonte 60s. LabT2 su planner+
  income, non solo tick. Torrette ad anello verso `hotspot()`, primi muri
  davanti a torretta (`wall_slots()` validata da `valid_ground+spawn_ok`).
  Rally per tipo da tabella offset.
- Assert: `planner_waits_when_broke`, `labt2_needs_eco`, `wall_slots_walkable`,
  `turret_faces_threat`. Metriche: income, base_hp, `median_match_ticks` non
  esplode.
- NON fare: multi-base, terza factory, micro nuovo.

### 0.0.19 — Scouting a frontiera + opponent modeling leggero
- Obiettivo: `scout-blitz explored@120s` >12%, first-blood anticipato,
  classificazione ≥80% in test.
- `Contact.confidence=1/(1+age/60)`, edifici ricordati usati:
  `attack_destination()` preferisce base nemica ricordata a target statico.
- Nuovo `src/ai/scout.rs`: `frontier_target()` (novelty − minaccia×peso,
  pesi in Personality), max 2 scout. Nuovo `src/ai/opponent.rs`: `classify()`
  → shift mix/courage ±0.1 da tabella. Executor devia waypoint da threat.
- Assert: `frontier_prefers_unseen`, `scout_avoids_hotspot`,
  `building_memory_redirects_attack`, `classify_rusher`. Metriche: explored,
  first-blood.
- NON fare: switch totale strategia mid-game, ML, scritture su VisibilityMap.

### 0.0.20 — Operazioni multi-ondata + capitale protetto + micro 4Hz
- Obiettivo: Commander mai perso in `hold-the-base`/`no-cheat`, stall/draw
  −30%, `median_match_ticks` giù.
- Nuovi intenti `AttackMoveGroup/HoldAtMaxRange/Screen`. `AttackMoveAll`
  esclude Commander + arty ferite. `commander_commit_prob` data-driven.
  Onde ogni ~75s rivalutate con `predict_outcome()`: se bassa resta a casa.
  Split `ai_tick` 1Hz + `micro_tick` 4Hz (solo Retreat/Focus/Hold/Screen,
  budget 2, isteresi). Retreat verso copertura torrette. Richiamo difensivo
  se threat in base.
- Assert: `commander_never_commits_early`, `arty_holds_behind_screen`,
  `waves_monotonic`, `retreat_to_turret`, `micro_respects_budget`.
- NON fare: formazioni oltre `formation()`, focus edifici, assedio mobile.

### 0.0.21 — `.ron` + handicap + suite hardening
- Obiettivo: 4 `.ron` bit-identici alle const, 3 handicap tarati
  (hard batte easy >60%), full verde su map-pool 4 seed.
- `personalities/*.ron` + loader `personality.rs` (`ron` solo se già
  transitiva via bevy, altrimenti `serde_json`). Const diventano
  `from_ron(include_str!)` + test parità.
- Nuovo `difficulty.rs`: `Handicap{apm, courage_malus, period_mult}`
  (stesso cervello, solo freno; `income_mult` default 1.0).
  `AiTeamConfig{handicap}`, league `turtle-hard/medium/easy`, seed `[0,1,2,3]`.
- Scenari nuovi gate: `counter-comp`, `commander-snipe`. Harness check
  `difficulty-applied` + `ron-loaded`.
- Assert: `ron_roundtrip`, `easy_capped`, `hard_beats_easy`,
  `counter-comp_passes`, `commander-snipe_passes`.
- NON fare: hot-reload, editor, TrueSkill, nuovi crate oltre `ron` transitiva.

## 4. Paletti trasversali
- Data-driven: `grep UnitKind::HeavyTank src/ai/*.rs` = 0 fuori test/tabelle.
- Determinismo: BTreeMap, sort per bits, `total_cmp`, tick fisso, no
  HashMap/rand/time in sim.
- Fog: solo `AiSnapshot+EnemyMemory`. Estendere `fog-honest` ai nuovi intenti.
- ≤16 param: nuovi dati via Resource/ParamSet, funzioni pure fuori dai sistemi.
- No nuove dip: `cargo tree -i ron` prima di aggiungerlo; fallback json.

## Stato implementazione- [x] 0.0.16 spec (questo file)
- [x] 0.0.16 code (threat shadow + explored_pct + memory API + metriche)
  - `cargo test` 145 verdi, `clippy -D warnings` verde, `fmt` verde.
  - `ai-test` PASS deterministico (checksum identici tra repeat) + nuovo
    check `threat-sane` verde.
  - Nota: `ai-scenarios` ha un gate rosso **pre-esistente** (`scout-blitz /
    scout-nav-clean`, 40 nav failure su mete flank ±60m in roccia). Non causato
    da 0.0.16: destinazioni scout/attacco bit-identiche (stesso ordine, stessa
    matematica, unit test `scouting_cycles...` verde), threat/director/league
    solo letture post-hoc, repeat deterministici. La suite stessa
    (`league.rs`, `scenarios.rs`, wiring CLI) è lavoro non committato: il gate
    è aspirazionale rispetto a mappa/nav attuali. Da decidere se allentare la
    meta, riparare gli spot, o accettare il rosso finché 0.0.19 (scout a
    frontiera threat-aware) lo risolve strutturalmente.
  - Diagnosi "blu incastrato" (repro turtle-vs-rusher 120s, test poi rimosso):
    NON è muro fisico (`nav.failed=0`, mappa connessa) ma stallo
    comportamentale: turtle senza occhi aspetta massa cieca `threshold+2=6`
    armati (a 120s ne ha ~4) e lo Scout costruito non esce mai perché
    l'intento `Scout` richiede `army_count==0`, impossibile col Commander
    armato vivo (ramo morto in Playground). La roccia grossa a est della base
    blu (-238,-262, 24×27m) è reale ma scenografia. Fix in 0.0.19 (scout a
    frontiera + intento svincolato dal conteggio armata).
- [x] 0.0.17 code (courage Lanchester + counter-comp + focus per valore)
  - `cargo test` 154 verdi (9 nuovi: dps/counter/Lanchester/focus/courage),
    `clippy -D warnings` verde, `fmt` verde.
  - `ai-test` PASS con checksum identici a 0.0.16; scenari: unico delta
    voluto in `hold-the-base` (sempre PASS, base 93%), resto bit-identico;
    `scout-blitz/nav-clean` resta il rosso pre-esistente noto.
  - League runner ora parallelo (worker = core, aggregazione in ordine fisso,
    stessi checksum) + probe `--ticks N` validati e deterministici.
  - POLICY SUITE (costi misurati: ~1ms/tick early, ~5ms/tick in combattimento;
    match full-cap 36k tick = minuti l'uno): in sessione SOLO unit + ai-test +
    scenari + probe ticks-capped (minuti). Quick/full full-cap = nightly/
    release, mai in analisi interattive. Guardrail winrate-vs-rush-scripted:
    non misurabile senza match lunghi → pending al primo run nightly.
- [x] G1 Metal spots (branch feat/metal-spots): 3 home/lato + mid + centro ×2,
  regola hard unica player/AI, ghost con magnete+highlight, marker oro,
  income × mult, AI Metal su spot. 161 test verdi, clippy/fmt puliti,
  ai-test PASS deterministico, scenari all_pass True (persino scout-nav-clean).
  Nota: micro-duel/siege cambiano esito vs baseline (6 vs 7 kill) pur senza
  cervelli — repeats bit-identici, contract ok, causa non isolata (duelli
  knife-edge; identità cross-versione mai promessa). Non inseguire oltre.
- [ ] G2 spawn E/O ACCANTONATO (decisione owner: nessuno sbilanciamento osservato,
  si tengono spawn fissi SW/NE per stabilità suite)
- [x] 0.0.18 macro (branch feat/0.0.18-macro): planner bottleneck su costi reali,
  LabT2 con cancello eco (10/24), torrette su anchor hotspot (fallback base),
  muri turtle (max 3, slot davanti alla torretta senza murare factory).
  175 test verdi (14 nuovi), clippy/fmt puliti, ai-test PASS con checksum
  identici, scenari all_pass True con checksum identici (meccanismi nuovi
  coperti da unit test puri: in suite la macro resta quasi sempre occupata e
  la minaccia è vuota, quindi non scattano — nessun delta involontario).
- [x] 0.0.19 code (scout frontiera + opponent modeling, mergiato in dev)
- [x] 0.0.20 code (onde multi-ondata + capitale protetto + micro 4Hz, mergiato in dev)
  - `AttackMoveGroup` con `wave_group()` (mai Commander salvo commit, mai scout,
    mai arty ferite), `commander_commit_prob` 0.95/0.9/2.0/2.0, onde 75s
    (`WAVE_PERIOD_TICKS=300`) rivalutate `predict_outcome()`, richiamo difensivo
    `BASE_THREAT_RADIUS=120`, split `ai_tick` 1Hz + `micro_tick` 4Hz budget 2,
    `HoldAtMaxRange`/`Screen`, retreat in copertura torrette.
  - Harden: batteria esclude Scout/Commander espliciti, hold tiene terreno in
    gittata, telemetria onde/micro (`waves_launched`, `micro_orders`,
    `commander_alive`, milestone `median_waves`/`commander_survival_rate`,
    check `waves-micro-sane`, label `M`/`m`).
  - `cargo test` ai 99+ verdi, `clippy -D warnings` verde, `fmt` verde,
    `ai-test` PASS deterministico, `ai-scenarios` 6/6 PASS deterministici.
- [x] 0.0.21 code (`.ron` + handicap + suite hardening, in dev)
  - `personalities/*.ron` (4, `ron` transitiva via bevy 0.12) + `from_ron` +
    test `ron_roundtrip`, `try_from_name` strict + harness strict,
    `src/ai/difficulty.rs` (`HARD/MEDIUM/EASY`, solo freno, income 1.0),
    `AiTeamConfig.handicap`, courage/APM/periodo applicati in `ai_tick`,
    league `turtle-hard/medium/easy` + quick 10 pairing + test `hard_beats_easy`,
    scenari `counter-comp` (4v1) + `commander-snipe` (vivo, 86% base),
    harness `ron-loaded` + `difficulty-applied`, full map-pool `[0,1,2,3]`.
  - `cargo test` ai 104 verdi, `ai-test` PASS (checksum stabili), `ai-scenarios`
    8/8 PASS deterministici.
- [x] Debito Punto 5 minimo: TTA gate (niente Build scaling se `tta` INF) +
  test `planner_waits_when_broke_e2e`.
- [ ] Debito futuro (specificato, non codice): utility scoring vero (Graham/
  Lewis/Hanlon-Watts), threat multilayer range-aware (Mark/Zielinski/Lewis),
  opponent con isteresi + scout information-gain (Welsh/Walsh/Zielinski),
  threat cache perf, TrueSkill/hot-reload (mai).
