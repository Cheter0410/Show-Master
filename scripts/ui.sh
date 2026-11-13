# Shared look for install / build / release scripts (source it, bash 3.2+).
#
#   ui_header "Show Master" "Release 0.4.1"   title box (skipped in nested scripts)
#   ui_section "Bauen"                        a group of steps
#   ui_run "Tests" cargo test ...             one live line while it runs: step number (3/7),
#                                             progress bar and time left (estimated from the
#                                             last run), and what it is doing right now with a
#                                             count (kompiliert egui 120/245); then ✓ / ✗ with
#                                             the time. Output goes to the log; on failure
#                                             the important lines are shown and the script ends
#   ui_skip "Tests" "schon geprüft"           a step that was not needed
#   ui_ok / ui_info / ui_warn / ui_fail       single lines (ui_fail ends the script)
#   UI_NAME=release.sh                        before sourcing: the script name for the step
#                                             total, when it runs from a temporary copy
#   ui_sudo                                   ask for the sudo password up front (a
#                                             password prompt inside ui_run is invisible)
#
# The log is shared with nested scripts through $UI_LOG.

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  UI_B=$'\033[1m' UI_D=$'\033[2m' UI_G=$'\033[32m' UI_R=$'\033[31m' UI_Y=$'\033[33m' UI_C=$'\033[36m' UI_0=$'\033[0m'
  UI_TTY=1
else
  UI_B='' UI_D='' UI_G='' UI_R='' UI_Y='' UI_C='' UI_0='' UI_TTY=
fi
if [ -z "${UI_LOG:-}" ]; then
  UI_LOG="${TMPDIR:-/tmp}/$(basename "$(pwd)")-$(date +%Y%m%d-%H%M%S).log"
  export UI_LOG
  : >"$UI_LOG"
  UI_TOP=1
else
  UI_TOP=
fi

ui_time() {
  local s=$1
  if [ "$s" -ge 60 ]; then printf '%d:%02d min' $((s / 60)) $((s % 60)); else printf '%d s' "$s"; fi
}

ui_header() {
  [ -n "$UI_TOP" ] || return 0
  local title="$1" sub="${2:-}"
  printf '\n%s%s%s' "$UI_B" "$title" "$UI_0"
  [ -n "$sub" ] && printf '  %s%s%s' "$UI_D" "$sub" "$UI_0"
  printf '\n%s%s%s\n' "$UI_D" "────────────────────────────────────────────" "$UI_0"
}

ui_section() { printf '\n%s%s%s\n' "$UI_B" "$1" "$UI_0"; }
ui_ok() { printf '  %s✓%s %s\n' "$UI_G" "$UI_0" "$*"; }
ui_info() { printf '  %s·%s %s\n' "$UI_D" "$UI_0" "$*"; }
ui_warn() { printf '  %s!%s %s\n' "$UI_Y" "$UI_0" "$*"; }
ui_skip() {
  ui_next_step
  printf '  %s↷ %-5s %s%s %s%s%s\n' "$UI_D" "$UI_STEP" "$1" "$UI_0" "$UI_D" "${2:+– $2}" "$UI_0"
}
ui_fail() {
  printf '\n  %s✗ %s%s\n' "$UI_R" "$*" "$UI_0"
  exit 1
}

ui_sudo() {
  [ "$(id -u)" = 0 ] && return 0
  sudo -n true 2>/dev/null && return 0
  printf '  %s🔑%s Administrator-Passwort (für die Installation):\n' "$UI_Y" "$UI_0"
  sudo -v || ui_fail "Ohne Administrator-Rechte geht es nicht weiter."
}

# How long steps took last time (lines of output, seconds, units): the progress bar.
UI_HIST="${XDG_CACHE_HOME:-$HOME/.cache}/script-steps"

ui_clock() { printf '%d:%02d' $(($1 / 60)) $(($1 % 60)); }

# What a line of output says, in a few words.
ui_activity() {
  printf '%s' "$1" | sed -E $'s/\033\\[[0-9;?]*[A-Za-z]//g; s/.*\r//; s/^[[:space:]]+//; s/[[:space:]]+$//' | sed -E \
    -e 's/^Compiling ([^ ]+).*/kompiliert \1/' \
    -e 's/^(Downloaded|Downloading) ([^ ]+) v.*/lädt \2/' \
    -e 's/^(Updating|Downloading) crates.*/lädt Bibliotheken/' \
    -e 's/^Running (unittests )?([^ ]+).*/Tests: \2/' \
    -e 's/^test ([^ ]+) \.\.\. .*/Test \1/' \
    -e 's/^Finished .*/fertig, schreibt das Programm/' \
    -e 's/^test result: .*/Tests ausgewertet/'
}

# Steps are told apart by label and folder (without numbers: versions and sizes change).
ui_step_key() {
  printf '%s-%s' "$(basename "$(pwd)")" "$1" | tr -d '0-9' | tr -c 'A-Za-z_.-' '_' | cut -c1-80
}

# Step numbers ("3/7"): counted across nested scripts (one character per step in a
# shared file); the total is how many steps the same command had last time.
if [ -n "$UI_TOP" ]; then
  UI_STEPS="$UI_LOG.steps"
  : >"$UI_STEPS"
  UI_RUN_KEY="run_$(ui_step_key "${UI_NAME:-$(basename "$0")} $*")"
  UI_TOTAL=0
  [ -f "$UI_HIST/$UI_RUN_KEY" ] && read -r UI_TOTAL <"$UI_HIST/$UI_RUN_KEY" 2>/dev/null
  case "$UI_TOTAL" in '' | *[!0-9]*) UI_TOTAL=0 ;; esac
  export UI_STEPS UI_TOTAL
fi

# Count this step; sets UI_STEP to "3/7" (or "3" while the total is not known).
ui_next_step() {
  local n=1
  if [ -n "${UI_STEPS:-}" ]; then
    printf x >>"$UI_STEPS" 2>/dev/null
    n=$(wc -c <"$UI_STEPS" 2>/dev/null | tr -d ' ')
  fi
  if [ "${UI_TOTAL:-0}" -ge "$n" ] 2>/dev/null; then UI_STEP="$n/$UI_TOTAL"; else UI_STEP="$n"; fi
}

# Things a step works through (crates compiled, tests run): counted for "120/245".
UI_UNITS='^[[:space:]]*Compiling |^test .* \.\.\. '

ui_bar() {
  local fill=$(($1 * $2 / 100)) a='' b='' i
  for ((i = 0; i < fill; i++)); do a+='━'; done
  for ((i = fill; i < $2; i++)); do b+='─'; done
  printf '%s%s%s%s%s' "$UI_C" "$a" "$UI_D" "$b" "$UI_0"
}

# ui_run "Label" command args...
ui_run() {
  local label="$1" start=$SECONDS rc=0 pid from
  shift
  printf '\n=== %s\n' "$label" >>"$UI_LOG"
  from=$(($(wc -l <"$UI_LOG") + 1))
  local hist prev_l=0 prev_s=0 prev_u=0 step
  hist="$UI_HIST/$(ui_step_key "$label")"
  [ -f "$hist" ] && read -r prev_l prev_s prev_u <"$hist" 2>/dev/null
  case "$prev_l$prev_s" in *[!0-9]*) prev_l=0 prev_s=0 ;; esac
  case "$prev_u" in '' | *[!0-9]*) prev_u=0 ;; esac
  ui_next_step
  step=$(printf '%-5s' "$UI_STEP")
  if [ -n "$UI_TTY" ]; then
    local cols el n u pct eta left act line plain room barw=18
    cols=$(tput cols 2>/dev/null || echo 80)
    [ "${cols:-0}" -gt 30 ] 2>/dev/null || cols=80
    ("$@") >>"$UI_LOG" 2>&1 &
    pid=$!
    while kill -0 "$pid" 2>/dev/null; do
      el=$((SECONDS - start))
      n=$(($(wc -l <"$UI_LOG") - from + 1))
      # grep -c exits 1 when it counts 0; under "set -e" that would end the script.
      u=$(tail -n +"$from" "$UI_LOG" | grep -cE "$UI_UNITS" || true)
      # Estimate: by crates / tests when there were some last time, by lines of
      # output when the step talks a lot, else by time.
      pct=
      if [ "$prev_u" -ge 5 ] && [ "$u" -le "$prev_u" ]; then
        pct=$((u * 100 / prev_u))
        [ "$pct" -le 99 ] || pct=99
      elif [ "$prev_s" -ge 3 ] && [ "$el" -le $((prev_s * 3 / 2 + 5)) ]; then
        if [ "$prev_l" -ge 10 ]; then
          pct=$((n * 100 / prev_l))
          [ "$pct" -le 120 ] || pct= # much more output than last time: no guess
        else
          pct=$((el * 100 / prev_s))
        fi
        [ -z "$pct" ] || [ "$pct" -le 99 ] || pct=99
      fi
      act=
      [ "$n" -gt 0 ] && act=$(ui_activity "$(tail -n 1 "$UI_LOG")")
      if [ "$u" -gt 0 ]; then
        if [ "$prev_u" -ge "$u" ]; then act="$u/$prev_u · $act"; else act="$u · $act"; fi
      fi
      if [ -n "$pct" ]; then
        # Early on the time so far says little: use last time's length.
        if [ "$pct" -ge 5 ] && [ "$el" -ge 3 ]; then
          eta=$((el * (100 - pct) / pct))
        else
          eta=$((prev_s * (100 - pct) / 100))
        fi
        if [ "$eta" -gt 0 ]; then left="noch ~$(ui_clock "$eta")"; else left="gleich fertig"; fi
        plain="  ▸ $step $label  $(printf '%*s' $barw '') ${pct}%  $(ui_clock "$el")  $left"
        line="  ${UI_C}▸${UI_0} ${UI_C}$step${UI_0} $label  $(ui_bar "$pct" $barw) ${UI_B}${pct}%${UI_0}  ${UI_D}$(ui_clock "$el")  $left${UI_0}"
      else
        plain="  ▸ $step $label  $(ui_clock "$el")"
        line="  ${UI_C}▸${UI_0} ${UI_C}$step${UI_0} $label  ${UI_D}$(ui_clock "$el")${UI_0}"
      fi
      room=$((cols - ${#plain} - 4))
      if [ -n "$act" ] && [ "$room" -gt 8 ]; then
        [ ${#act} -le "$room" ] || act="${act:0:$((room - 1))}…"
        line="$line  ${UI_D}·${UI_0} $act"
      fi
      printf '\r\033[K%s' "$line"
      sleep 0.25
    done
    wait "$pid" || rc=$?
    printf '\r\033[K'
  else
    printf '  … %s %s\n' "$UI_STEP" "$label"
    ("$@") >>"$UI_LOG" 2>&1 || rc=$?
  fi
  if [ "$rc" -eq 0 ]; then
    mkdir -p "$UI_HIST" 2>/dev/null &&
      printf '%s %s %s\n' "$(($(wc -l <"$UI_LOG") - from + 1))" "$((SECONDS - start))" \
        "$(tail -n +"$from" "$UI_LOG" | grep -cE "$UI_UNITS")" >"$hist" 2>/dev/null
    printf '  %s✓%s %s%s%s %s %s(%s)%s\n' "$UI_G" "$UI_0" "$UI_D" "$step" "$UI_0" "$label" "$UI_D" "$(ui_time $((SECONDS - start)))" "$UI_0"
  else
    printf '  %s✗ %s %s%s\n\n' "$UI_R" "$step" "$label" "$UI_0"
    # The interesting part: errors and failed tests, else the last lines.
    local out
    out=$(tail -n +"$from" "$UI_LOG")
    if printf '%s\n' "$out" | grep -qE '^(error|test .* FAILED|---- )'; then
      printf '%s\n' "$out" | grep -E '^(error|test .* FAILED|---- )|panicked at' -A3 | tail -n 40 | sed 's/^/    /'
    else
      printf '%s\n' "$out" | tail -n 25 | sed 's/^/    /'
    fi
    printf '\n  %sGanzes Protokoll: %s%s\n' "$UI_D" "$UI_LOG" "$UI_0"
    exit 1
  fi
}

# Fingerprint of files (to tell whether an output is still current).
ui_fingerprint() {
  if command -v shasum >/dev/null 2>&1; then
    cat "$@" | shasum -a 256 | cut -c1-16
  else
    cat "$@" | sha256sum | cut -c1-16
  fi
}

# ui_fresh <output> <key>: the output exists and was made from <key>.
ui_fresh() {
  [ -f "$1" ] && [ -f "$(dirname "$1")/.made/$(basename "$1")" ] &&
    [ "$(cat "$(dirname "$1")/.made/$(basename "$1")")" = "$2" ]
}
# ui_made <output> <key>: remember what the output was made from.
ui_made() {
  mkdir -p "$(dirname "$1")/.made"
  printf '%s' "$2" >"$(dirname "$1")/.made/$(basename "$1")"
}

ui_done() {
  [ -n "$UI_TOP" ] || return 0
  # Remember how many steps this command had, for "3/7" next time.
  mkdir -p "$UI_HIST" 2>/dev/null &&
    wc -c <"$UI_STEPS" 2>/dev/null | tr -d ' ' >"$UI_HIST/$UI_RUN_KEY" 2>/dev/null
  rm -f "$UI_STEPS"
  printf '\n%s%s%s\n' "$UI_D" "Protokoll: $UI_LOG" "$UI_0"
}
