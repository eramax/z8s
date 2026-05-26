#!/usr/bin/env bash
# z8s daemon control script
set -uo pipefail

PIDFILE="/tmp/z8s.pid"
LOGFILE="/tmp/z8s.log"
BINARY="/home/abb/dev/z8s/target/debug/z8s"
WORKDIR="/home/abb/dev/z8s"

build() {
    echo "Building z8s..."
    cd "$WORKDIR"
    cargo build 2>&1
    echo "Build complete: $BINARY"
}

start() {
    if [ -f "$PIDFILE" ]; then
        local pid; pid=$(cat "$PIDFILE" 2>/dev/null)
        if [ -n "$pid" ] && sudo kill -0 "$pid" 2>/dev/null; then
            echo "z8s already running (PID $pid, log=$LOGFILE)"
            exit 1
        fi
        sudo rm -f "$PIDFILE"
    fi
    if sudo pidof -s z8s >/dev/null 2>&1; then
        echo "z8s already running (stale pidfile but process alive)"
        sudo pidof z8s || true
        exit 1
    fi
    cd "$WORKDIR"
    sudo rm -f "$LOGFILE"
    # Start z8s under sudo, capture the sudo process PID
    sudo "$BINARY" "$@" >>"$LOGFILE" 2>&1 &
    local sudo_pid=$!
    sleep 0.5
    # Find the actual z8s child PID
    local z8s_pid
    z8s_pid=$(sudo pgrep -P "$sudo_pid" 2>/dev/null | head -1)
    if [ -n "$z8s_pid" ]; then
        echo "$z8s_pid" > "$PIDFILE"
        echo "z8s started (PID $z8s_pid, log=$LOGFILE)"
    else
        # Fallback: sudo may have already exec'd — try pidof
        z8s_pid=$(sudo pidof -s z8s 2>/dev/null || true)
        if [ -n "$z8s_pid" ]; then
            echo "$z8s_pid" > "$PIDFILE"
            echo "z8s started (PID $z8s_pid, log=$LOGFILE)"
        else
            echo "z8s failed to start - check $LOGFILE"
            tail -20 "$LOGFILE" >&2 || true
            sudo rm -f "$PIDFILE"
            exit 1
        fi
    fi
}

stop() {
    local target_pid=""
    # Find actual z8s process first (most reliable)
    target_pid=$(sudo pidof -s z8s 2>/dev/null || true)
    # Fallback to pidfile (which may be the sudo wrapper PID)
    if [ -z "$target_pid" ] && [ -f "$PIDFILE" ]; then
        target_pid=$(cat "$PIDFILE" 2>/dev/null || true)
    fi
    if [ -n "$target_pid" ]; then
        echo "Stopping z8s (PID $target_pid)..."
        sudo kill -TERM "$target_pid" 2>/dev/null || true
        for i in $(seq 1 10); do
            if ! sudo kill -0 "$target_pid" 2>/dev/null; then
                break
            fi
            sleep 1
        done
        if sudo kill -0 "$target_pid" 2>/dev/null; then
            echo "Force killing..."
            sudo kill -KILL "$target_pid" 2>/dev/null || true
        fi
    fi
    sudo rm -f "$PIDFILE"
    echo "z8s stopped"
}

restart() {
    stop
    sleep 1
    start "$@"
}

status() {
    if [ -f "$PIDFILE" ]; then
        local pid; pid=$(cat "$PIDFILE" 2>/dev/null)
        if [ -n "$pid" ] && sudo kill -0 "$pid" 2>/dev/null; then
            echo "z8s running (PID $pid, log=$LOGFILE)"
            return
        fi
    fi
    local pid; pid=$(sudo pidof -s z8s 2>/dev/null || true)
    if [ -n "$pid" ]; then
        echo "z8s running (no pidfile, PID $pid)"
    else
        echo "z8s not running"
    fi
}

logs() {
    local lines="${2:-50}"
    tail -n "$lines" -f "$LOGFILE"
}

case "${1:-status}" in
    start)   shift; start "$@" ;;
    stop)    stop ;;
    restart) shift; restart "$@" ;;
    status)  status ;;
    build)   build ;;
    logs)    logs "$@" ;;
    *)       echo "Usage: $0 {start|stop|restart|status|build|logs} [z8s-flags...]" >&2; exit 1 ;;
esac
