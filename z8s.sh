#!/usr/bin/env bash
set -e

PIDFILE="/tmp/z8s.pid"
LOGFILE="/tmp/z8s.log"
BINARY="/home/abb/dev/z8s/target/debug/z8s"
WORKDIR="/home/abb/dev/z8s"

start() {
    if [ -f "$PIDFILE" ] && sudo kill -0 "$(sudo cat "$PIDFILE")" 2>/dev/null; then
        echo "z8s already running (PID $(sudo cat "$PIDFILE"))"
        exit 1
    fi
    cd "$WORKDIR"
    sudo touch "$LOGFILE"
    sudo "$BINARY" >> "$LOGFILE" 2>&1 &
    PID=$!
    echo "$PID" | sudo tee "$PIDFILE" > /dev/null
    sleep 1
    if sudo kill -0 "$PID" 2>/dev/null; then
        echo "z8s started (PID $PID)"
    else
        echo "z8s failed to start - check $LOGFILE"
        sudo rm -f "$PIDFILE"
        exit 1
    fi
}

stop() {
    if [ -f "$PIDFILE" ]; then
        PID=$(sudo cat "$PIDFILE")
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
        sudo rm -f "$PIDFILE"
    else
        # fallback: kill any z8s
        sudo pkill -f "^sudo.*$BINARY" 2>/dev/null || true
    fi
    echo "z8s stopped"
}

restart() {
    stop
    sleep 1
    start
}

status() {
    if [ -f "$PIDFILE" ] && sudo kill -0 "$(sudo cat "$PIDFILE")" 2>/dev/null; then
        echo "z8s running (PID $(sudo cat "$PIDFILE"))"
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
