#!/usr/bin/env bash
# offside: test lunghi in background (detached) + retrieval dopo.
#
# I check veloci (fmt, clippy, `cargo test ai::`) restano in sessione.
# Quelli lunghi (suite completa, ai-test, scenari, torneo) vanno qui:
# la shell li lancia e tu continui a sviluppare; i risultati li recuperi dopo.
#
# Uso:
#   scripts/offside.sh run <nome> [--dir <repo>] -- "<comando>"
#   (il comando è UNA stringa: quotata se contiene pipe/redirect/&&)
#   scripts/offside.sh status [nome]     # vivo? da quanto? ultime righe utili
#   scripts/offside.sh get <nome>        # riepilogo completo a fine run
#   scripts/offside.sh log <nome> [n]    # ultime n righe di log (default 15)
#   scripts/offside.sh cancel <nome>     # ferma il run
#   scripts/offside.sh forget <nome>     # dimentica il run (pulisce i file)
#   scripts/offside.sh list              # tutti i run registrati
#
# Ogni run vive in benchmark-results/offside/<nome>/ :
#   manifest.json  (comando, pid, log, inizio)
#   run.log        (output completo)
#   EXIT           (exit code, solo a run finito)
# Esempio:
#   scripts/offside.sh run gates-merge -- ./scripts/test-suite.sh
#   ... sviluppi altro ...
#   scripts/offside.sh status gates-merge
#   scripts/offside.sh get gates-merge
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BASE="$ROOT/benchmark-results/offside"

usage() { sed -n '2,/^set -u/p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }

run_dir() { printf '%s/%s' "$BASE" "$1"; }
is_alive() { kill -0 "$1" 2>/dev/null; }

# kill ricorsivo dell'albero (wrapper -> bash cmd.sh -> cargo -> gioco):
# pkill -P prende solo i figli diretti, qui scendiamo fino in fondo.
killtree() {
    local p="$1" c
    for c in $(pgrep -P "$p" 2>/dev/null); do killtree "$c"; done
    kill "$p" 2>/dev/null
}

cmd_run() {
    local name="$1"; shift
    local dir="$ROOT"
    while [ "${1:-}" != "--" ]; do
        case "${1:-}" in
            --dir) dir="$2"; shift 2 ;;
            *) echo "flag sconosciuta: $1 (uso: run <nome> [--dir <repo>] -- <cmd>)"; exit 1 ;;
        esac
    done
    shift # via '--'
    [ "$#" -gt 0 ] || { echo "manca il comando"; exit 1; }
    local rd; rd="$(run_dir "$name")"
    if [ -d "$rd" ] && [ -f "$rd/manifest.json" ]; then
        local oldpid; oldpid="$(grep -o '"pid":[0-9]*' "$rd/manifest.json" | grep -o '[0-9]*')"
        if [ -n "${oldpid:-}" ] && is_alive "$oldpid" && [ ! -f "$rd/EXIT" ]; then
            echo "run '$name' già attivo (pid $oldpid). status/cancel prima di rilanciarlo."
            exit 1
        fi
        rm -rf "$rd"
    fi
    mkdir -p "$rd"
    # --dir deve essere una repo col manifest giusto (evita log orfani)
    [ -f "$dir/Cargo.toml" ] || { echo "dir non valida: $dir"; exit 1; }
    local started; started="$(date '+%Y-%m-%d %H:%M:%S %z')"
    # Il comando va in cmd.sh (una sola stringa, quotata dal chiamante se
    # contiene pipe/redirect); eseguito dentro --dir. Il wrapper salva
    # l'exit code in EXIT e, se killato, marca 143 (cancel).
    printf 'cd %q && %s\n' "$dir" "$*" > "$rd/cmd.sh"
    chmod +x "$rd/cmd.sh"
    # wrapper: esegue cmd.sh, salva l'exit code in EXIT.
    # (niente setsid: non esiste su macOS; nohup+disown basta: il processo
    # sopravvive alla shell come le probe lanciate finora.)
    nohup bash -c 'RD="$0"; trap "echo 143 > \"$RD/EXIT\"" TERM INT; bash "$RD/cmd.sh"; code=$?; echo "$code" > "$RD/EXIT"' "$rd" >"$rd/run.log" 2>&1 < /dev/null &
    disown 2>/dev/null || true
    local pid=$!
    printf '{"name":"%s","dir":"%s","pid":%s,"log":"run.log","started":"%s","cmd":%s}\n' \
        "$name" "$dir" "$pid" "$started" "$(printf '%s' "$*" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')" \
        > "$rd/manifest.json"
    sleep 1
    if [ -f "$rd/EXIT" ]; then
        echo "run '$name' già finito (exit $(cat "$rd/EXIT")). vedi $rd/run.log"
        return 0
    fi
    if ! is_alive "$pid"; then
        echo "partenza fallita, vedi $rd/run.log"; tail -n 5 "$rd/run.log"; exit 1
    fi
    echo "run '$name' lanciato (pid $pid). log: $rd/run.log"
}

run_state() { # $1=name -> RUNNING <pid> | DONE <code> | UNKNOWN
    local rd; rd="$(run_dir "$1")"
    [ -f "$rd/manifest.json" ] || { echo "UNKNOWN"; return; }
    if [ -f "$rd/EXIT" ]; then echo "DONE $(cat "$rd/EXIT")"; return; fi
    local pid; pid="$(grep -o '"pid":[0-9]*' "$rd/manifest.json" | grep -o '[0-9]*')"
    if [ -n "${pid:-}" ] && is_alive "$pid"; then echo "RUNNING $pid"; else echo "DONE ?"; fi
}

cmd_status() {
    if [ "$#" -eq 0 ]; then
        for d in "$BASE"/*/; do
            [ -d "$d" ] || continue
            printf '%-22s %s\n' "$(basename "$d"):" "$(run_state "$(basename "$d")")"
        done
        return
    fi
    local rd; rd="$(run_dir "$1")"
    [ -f "$rd/manifest.json" ] || { echo "run '$1' inesistente"; exit 1; }
    echo "== $1: $(run_state "$1") =="
    grep -o '"cmd":"[^"]*"' "$rd/manifest.json" | head -n 1
    echo "--- segnali nel log ---"
    grep -cE 'test result: ok' "$rd/run.log" 2>/dev/null | xargs -I{} echo "suite ok: {}"
    grep -E 'test result: FAILED|FAILED|panicked|^error(\[|:)|FAIL' "$rd/run.log" | head -n 5
    grep -E '^league .* repeat=' "$rd/run.log" | tail -n 3
    echo "--- coda log ---"
    tail -n 5 "$rd/run.log"
}

cmd_get() {
    local rd; rd="$(run_dir "$1")"
    [ -f "$rd/manifest.json" ] || { echo "run '$1' inesistente"; exit 1; }
    cat "$rd/manifest.json"; echo
    echo "stato: $(run_state "$1")"
    echo "--- conteggi ---"
    echo "test-ok: $(grep -c 'test result: ok' "$rd/run.log" 2>/dev/null)"
    echo "fail/panic/error: $(grep -cE 'test result: FAILED|panicked|^error(\[|:)' "$rd/run.log" 2>/dev/null)"
    echo "match torneo finiti: $(grep -cE '^league .* repeat=' "$rd/run.log" 2>/dev/null)"
    echo "--- report trovati ---"
    find "$ROOT/benchmark-results" -maxdepth 2 -newer "$rd/manifest.json" \( -name '*.json' -o -name '*.md' \) 2>/dev/null | head -n 10
    echo "--- ultimi check PASS/FAIL ---"
    grep -E '^\s+\[(ok|FAIL)\]' "$rd/run.log" | tail -n 20
}

case "${1:-}" in
    run) shift; cmd_run "$@" ;;
    status) shift; cmd_status "$@" ;;
    get) shift; [ "$#" -eq 1 ] || usage 1; cmd_get "$1" ;;
    log) shift; tail -n "${3:-15}" "$(run_dir "$1")/run.log" ;;
    forget) shift; rm -rf "$(run_dir "$1")"; echo "dimenticato $1" ;;
    cancel) shift; p="$(grep -o '"pid":[0-9]*' "$(run_dir "$1")/manifest.json" | grep -o '[0-9]*')"; killtree "$p"; echo "fermato $1 (albero pid $p)" ;;
    list) shift; cmd_status ;;
    *) usage ;;
esac
