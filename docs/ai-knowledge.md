# AI knowledge base — indice versionato (spec eseguibile)

Questo file è la **source of truth versionata** per le fonti AI. Le sintesi di studio
stanno nel vault Obsidian locale sister `../RRTS-AI-Vault` (non in git, futuro repo
a parte). Qui resta solo ciò che serve a build/test/review.

Regola ibrida: `docs/` = cosa deve fare il codice (Obiettivo/Assert/Metriche/NON-fare),
vault = perché / cosa imparato / cosa scartato. Il vault linka al codice con
`percorso:linea`, mai copia-incolla. Mai PDF in repo o vault, solo URL.

Spec madre: `docs/ai-advanced-plan.md` (fasi 0.0.18 → 0.0.21, paletti trasversali).
Dettagli per fase: `docs/ai/018-macro-planner.md`, `019-scout-opponent.md`,
`021-difficulty-suite.md`, `022-utility-full.md`, `023-threat-multilayer.md`.

## Fonti Tier0 (leggere prima, tutte gratuite su gameaipro.com)

- Utility: Graham Pro1 Ch9 (theory), Lewis Pro3 Ch13 (considerations), Hanlon/Watts Pro3 Ch31 (DA:I).
- Threat: Mark Pro2 Ch30 (modular), Zielinski Pro3 Ch24 (where it counts), Lewis Pro2 Ch29 (piano B continuo).
- Combat: Stanescu/Barriga/Buro Pro3 Ch25 (Lanchester fast-forward, già in `src/ai/combat.rs`).
- Macro: Foged/Horswill Pro2 Ch26 (constraint greedy), Johnson Pro3 Ch26 (spatial queries).
- Scout: Welsh Pro2 Ch27 (search), Walsh Pro2 Ch28 (perception/confidence), Zielinski Pro1 Ch33 (smart questions).
- Tuning: Borovikov Online Ch10 (autoplay per handicap).

URL completi + note di applicabilità: vault `90-Riferimenti/90-Bibliografia.md`.
Se il vault non c'è, gli URL sopra restano risolvibili da `https://www.gameaipro.com/`.

## Fonti Tier1 (riferimento architettura/evaluation)

- UAlbertaBot wiki (bot modulare BWAPI), StarCraft AI Competition AIIDE/CoG (ladder/mirror-bias/map-pool),
  MicroRTS (greedy + competizione), BAR guides (semantica AttackMove/Guard/Hold già imitata),
  Bevy Learn + docs.rs (ParamSet/Resource per ≤16 param).

## Paletti (ripetuti per non perderli)

Determinismo bit-identico (BTreeMap, sort per bits, `total_cmp`, tick fisso, no
HashMap/rand/time in sim). Fog solo via `AiSnapshot + EnemyMemory` (gate `fog-honest`).
Max 16 param Bevy (pure fn fuori). No nuove dip senza motivo. Data-driven
(`grep UnitKind::HeavyTank src/ai/*.rs` = 0 fuori test/tabelle). Gate ogni fase:
`cargo test + ai-test + ai-suite quick + ai-scenarios` + delta checksum/winrate dichiarato prima.
Match full-cap 36k tick solo nightly; in sessione probe ticks-capped.

## Scartati (non riaprire senza motivo forte)

MCTS/HTN online, reti/RL, flow field, GPGPU, multi-base/terza factory in 0.0.18-0.0.20,
timer anti-stallo per i main. Motivi: vault `90-Riferimenti/91-Scartati.md`.
