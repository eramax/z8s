#!/usr/bin/env bash
set -e

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
    if pgrep -f "$BINARY" >/dev/null 2>&1; then
        echo "z8s already running (use: $0 stop)"
        pgrep -af "$BINARY" || true
        exit 1
    fi
    rm -f "$PIDFILE"
    cd "$WORKDIR"
    touch "$LOGFILE"
    set -m
    sudo "$BINARY" "$@" >> "$LOGFILE" 2>&1 &
    set +m
    PID=$!
    echo "$PID" > "$PIDFILE"
    sleep 1
    if kill -0 "$PID" 2>/dev/null; then
        echo "z8s started (PID $PID, log=$LOGFILE)"
    else
        echo "z8s failed to start - check $LOGFILE"
        tail -20 "$LOGFILE" >&2
        rm -f "$PIDFILE"
        exit 1
    fi
}

stop() {
    if [ -f "$PIDFILE" ]; then
        PID=$(cat "$PIDFILE")
        echo "Stopping z8s (PID $PID)..."
        sudo kill -TERM "$PID" 2>/dev/null || true
        for i in $(seq 1 10); do
            if ! sudo kill -0 "$PID" 2>/dev/null; then
                break
            fi
            sleep 1
        done
        if sudo kill -0 "$PID" 2>/dev/null; then
            echo "Force killing..."
            sudo kill -KILL "$PID" 2>/dev/null || true
        fi
        rm -f "$PIDFILE"
    fi
    if pgrep -f "$BINARY" >/dev/null 2>&1; then
        echo "Stopping stale z8s processes..."
        sudo pkill -TERM -f "$BINARY" 2>/dev/null || true
        sleep 1
        sudo pkill -KILL -f "$BINARY" 2>/dev/null || true
    fi
    echo "z8s stopped"
}

restart() {
    stop
    sleep 1
    start "${@:2}"
}

status() {
    if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
        echo "z8s running (PID $(cat "$PIDFILE"), log=$LOGFILE)"
    else
        echo "z8s not running"
    fi
}

logs() {
    LINES="${2:-50}"
    tail -n "$LINES" -f "$LOGFILE"
}

case "${1:-status}" in
    start)   shift; start "$@" ;;
    stop)    stop ;;
    restart) shift; stop; sleep 1; start "$@" ;;
    status)  status ;;
    build)   build ;;
    logs)    logs "$@" ;;
    *)       echo "Usage: $0 {start|stop|restart|status|build|logs} [z8s-flags...]" >&2; exit 1 ;;
esac
