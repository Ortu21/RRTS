# 0.0.22 — Utility scoring full (spec snella)

Madre: `docs/ai-advanced-plan.md` §4 debito futuro (utility scoring vero Graham/
Lewis/Hanlon-Watts). Studio: vault `10-Utility-AI/`. Fonti: Graham Pro1 Ch9,
Lewis Pro3 Ch13, Hanlon/Watts Pro3 Ch31.

## Obiettivo

Ogni macro-azione parallela (Build/Enqueue/Attack/Scout/Defense) ha uno scorer
`0..1` puro + pesi in `Personality/.ron`. `decide()` non ha più rami `if/else`
secchi: ogni blocco è gated da `score × peso > soglia`. A pesi default il
comportamento è bit-identico (continuità), ma le curve smussano i knife-edge
e la telemetria espone gli score per il tuning futuro.

Parallelo, non winner-take-all (DA:I per azioni singole, qui macro parallela:
ogni categoria ha il suo threshold, come `courage` per l'attacco).

## Cosa fare

- `src/ai/utility.rs`: nuovi scorer puri `build_urgency()`, `enqueue_urgency()`,
  `scout_urgency()`, `defense_urgency()` + `UtilityScores` per telemetria.
  Solo aritmetica, `clamp01/smoothstep`, niente `HashMap`/rand/time.
- `src/ai/strategy.rs`: `Personality` += 4 pesi `w_build/w_enqueue/w_scout/
  w_defense` (default 1.0), `PersonalityDef` + `from_ron` + const + `.ron`.
  `decide()` calcola gli score e skirt i blocchi sotto soglia `0.05`
  (permissiva: a default emette come prima, salta solo lo zero vero).
- `src/ai/director.rs`: check descrittivo `utility-sane` (score finiti 0..1).
- `src/ai/debug.rs`: label `U{attack,build,enq,scout,def}` negli intent debug.
- `personalities/*.ron`: 4 pesi aggiunti (1.0 default).

## Assert / metriche

`build_bootstrap_is_one`, `build_no_deficit_is_zero`, `enqueue_full_is_zero`,
`scout_blind_is_high`, `defense_threatened_is_one`, `ron_roundtrip` (4 .ron con
pesi), `utility_scores_bounded`. Metriche: `ai-test` PASS checksum dichiarati
(identici attesi), `ai-scenarios` 8/8 PASS, `median_match_ticks` non esplode.

## NON fare

Winner-take-all singolo, kiting nuovo, threat multistrato, opponent isteresi,
TrueSkill/hot-reload, nuove dipendenze.

## Delta atteso

Checksum identici a baseline (gate permissivi). Se qualche scenario cambia,
dichiarare prima quale e perché (solo zeri veri skippati).
