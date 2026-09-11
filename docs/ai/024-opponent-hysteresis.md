# 0.0.24 — Opponent con isteresi + scout information-gain (spec snella)

Madre: `docs/ai-advanced-plan.md` §4 debito futuro (opponent isteresi +
scout information-gain Welsh/Walsh/Zielinski). Studio: vault
`50-Scouting-Opponent/`. Fonti: Zielinski Pro1 Ch33 (smart questions:
conteggi + timing + edifici), Walsh Pro2 Ch28 (perception/confidence), Welsh
Pro2 Ch27 (search realistico: valore = cosa impari).

## Obiettivo

`classify()` istantaneo → credenza con isteresi (3s di letture concordi
prima di cambiare classe) + confidence 0..1 (classe nota × esplorato × occhi
freschi) che pesa gli shift (stessa classe a confidence 1 = stesso
comportamento di prima). Scout: novelty binaria → information-gain attesa
(frazione inesplorata nel 3×3: 1.0 = tutto da scoprire, agli estremi coincide
con la vecchia). Nuovo scenario `arty-siege` range-sensibile (batteria 30m vs
linea 13m: esercita lo spread 0.0.23, oggi nessun gate lo distingue).

## Cosa fare

- `src/ai/opponent.rs`: `OpponentBelief{stable,candidate,streak}` (Default =
  Unknown), `FLIP_AFTER=12` (≈3s a cadence snapshot 4Hz), `update_belief()`
  pura, `belief_confidence()` 0..1, `adjust_courage_conf()` /
  `mix_bias_conf()` (conf=1 → identici a `adjust_courage`/`mix_bias`).
  `OpponentKind: Default` (Unknown). `classify()` invariato (lettura grezza).
- `src/ai/snapshot.rs`: `AiSnapshot` += `opponent` + `opp_confidence`
  (Default Unknown/0); `refresh_snapshots` (+1 param `ResMut<AiState>`, resta
  ≤16) aggiorna la credenza e la stampa nello snapshot: la strategia legge
  SOLO percezione, mai stato (stessa architettura della threat cache).
- `src/ai/mod.rs`: `AiState` += `opponent_belief: BTreeMap<u8, OpponentBelief>`
  (reset su `R` con lo stato: mai stale tra match).
- `src/ai/strategy.rs`: `decide()` usa `snapshot.opponent/confidence` pesati;
  nei test con snapshot a mano la credenza è Unknown come il raw dei casi
  esistenti — dove il raw era noto, il test stampa la credenza convergente
  (simula `refresh`: `opponent = classify(&snap), conf = 1.0`).
- `src/ai/scout.rs`: `info_gain()` + uso al posto della novelty binaria in
  `frontier_targets` (conferma contatti invariata).
- `src/ai/scenarios.rs`: `arty-siege` (3 arty blu ferme vs 4 light rossi in
  attack-move, 3600t, scriptato puro) + gate larghe calibrate sul run reale
  (sopravvissuti blu, perdite rosse, nav-clean, fog-honest) + registry +
  test batteria 9→10.
- `src/ai/director.rs`: check `opponent-sane` (classe valida, conf 0..1).

## Assert / metriche

`belief_flips_after_streak_not_before`, `belief_confidence_bounds`,
`weighted_matches_unweighted_at_full_confidence`,
`info_gain_matches_binary_at_extremes`, `classify_accuracy` resta ≥80%,
`arty-siege` PASS deterministico. Metriche: `scout-blitz explored@120s`,
`commander-snipe base_hp`, `hold-the-base hp>55%`.

## NON fare

Switch totale strategia mid-game, ML, scritture su `VisibilityMap`, nuovi
layer threat, TrueSkill/hot-reload, nuove dipendenze.

## Delta atteso (dichiarato prima)

Isteresi (lag 3s sui flip) + rerank frontiera cambiano decisioni AI-driven:
`scout-blitz`, `eco-race`, `no-cheat`, `commander-snipe`, `hold-the-base`,
`counter-comp` POSSONO cambiare checksum (sempre PASS + deterministici tra
repeat). Scriptati (`micro-duel`, `siege`, `lance-hold`) + `arty-siege`
(nuovo) identici per costruzione. Direzione attesa: niente flip-flicker su
contatti singoli, scout verso l'inesplorato profondo.
