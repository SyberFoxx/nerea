#!/bin/sh
# Push-to-talk for a running Nerea: one line into her named pipe.
#
#   nerea-talk.sh          toggle: press to start listening, press again to send
#   nerea-talk.sh start    start listening (hold-to-talk keybinds)
#   nerea-talk.sh stop     stop, transcribe and answer
#
# Bind it in your compositor. In the Ryoku Hyprland config:
#
#   hl.bind(K("SUPER + X"), hl.dsp.exec_cmd("$HOME/Proyectos/nerea/packaging/nerea-talk.sh"))
PIPE="${XDG_RUNTIME_DIR:-/tmp}/nerea/ptt"

if [ ! -p "$PIPE" ]; then
  echo "nerea no está corriendo (falta $PIPE)" >&2
  exit 1
fi

echo "${1:-toggle}" > "$PIPE"
