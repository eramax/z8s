#!/usr/bin/env bash
set -e

PIDFILE="/tmp/z8s.pid"
LOGFILE="/tmp/z8s.log"
BINARY="/home/abb/dev/z8s/target/debug/z8s"
WORKDIR="/home/abb/dev/z8s"

start() {
    if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
        echo "z8s already running (PID $(cat "$PIDFILE"))"
        exit 1
    fi
    cd "$WORKDIR"
    touch "$LOGFILE"
    "$BINARY" >> "$LOGFILE" 2>&1 &
    PID=$!
    echo "$PID" > "$PIDFILE"
    sleep 1
    if kill -0 "$PID" 2>/dev/null; then
        echo "z8s started (PID $PID, log=$LOGFILE)"
    else
        echo "z8s failed to start - check $LOGFILE"
        rm -f "$PIDFILE"
        exit 1
    fi
}

stop() {
    if [ -f "$PIDFILE" ]; then
        PID=$(cat "$PIDFILE")
        echo "Stopping z8s (PID $PID)..."
        kill -TERM "$PID" 2>/dev/null || true
        for i in $(seq 1 10); do
            if ! kill -0 "$PID" 2>/dev/null; then
                break
            fi
            sleep 1
        done
        if kill -0 "$PID" 2>/dev/null; then
            echo "Force killing..."
            kill -KILL "$PID" 2>/dev/null || true
        fi
        rm -f "$PIDFILE"
    else
        # fallback: kill any running z8s debug binary
        pkill -f "$BINARY" 2>/dev/null || true
    fi
    echo "z8s stopped"
}

restart() {
    stop
    sleep 1
    start
}

status() {
    if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
        echo "z8s running (PID $(cat "$PIDFILE"), log=$LOGFILE)"
    else
        echo "z8s not running"
    fi
}

case "${1:-status}" in
    start)   start ;;
    stop)    stop ;;
    restart) restart ;;
    status)  status ;;
    *)       echo "Usage: $0 {start|stop|restart|status}" >&2; exit 1 ;;
esac
