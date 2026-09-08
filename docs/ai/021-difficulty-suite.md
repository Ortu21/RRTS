# 0.0.21 — .ron + handicap + suite hardening (spec snella)

Madre: `docs/ai-advanced-plan.md` §3 (0.0.21). Studio: vault `10-Utility-AI/`,
`60-Evaluation/`. Fonti: Hanlon/Watts Pro3 Ch31, Borovikov Online Ch10, ladder AIIDE/CoG,
MicroRTS competition.

## Obiettivo

4 `.ron` bit-identici alle const, 3 handicap tarati (hard batte easy >60%), full verde
su map-pool 4 seed `[0,1,2,3]`.

## Cosa fare

- `personalities/*.ron` + loader (`ron` solo se già transitiva via bevy, altrimenti
  `serde_json`). Const = `from_ron(include_str!)` + test parità `ron_roundtrip`.
- Nuovo `src/ai/difficulty.rs`: `Handicap{apm, courage_malus, period_mult}` sullo stesso
  cervello, solo freno (`income_mult` default 1.0). `AiTeamConfig{handicap}`, league
  `turtle-hard/medium/easy`. Allinea `from_name()` a `resolve_brain()` strict.
- Scenari `counter-comp`, `commander-snipe`; harness `difficulty-applied` + `ron-loaded`.

## Assert / metriche

`ron_roundtrip`, `easy_capped`, `hard_beats_easy`, `counter-comp_passes`,
`commander-snipe_passes`.

## NON fare

Hot-reload, editor, TrueSkill, nuovi crate oltre `ron` transitiva.

## Delta atteso

Pari `.ron` = checksum identici; handicap = winrate ordinati hard>medium>easy.
