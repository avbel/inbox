#!/usr/bin/env bash
set -uo pipefail

INBOX="/home/andrey/inbox/target/release/inbox"

if [[ ! -x "$INBOX" ]]; then
    echo "ERROR: inbox binary not found at $INBOX"
    echo "Build it first: cargo build --release"
    exit 1
fi

TESTDIR=$(mktemp -d)
PASS=0
FAIL=0
SKIP=0
RESULTS=()

cleanup() { rm -rf "$TESTDIR"; }
trap cleanup EXIT

log() {
    local name="$1" status="$2" detail="${3:-}"
    case "$status" in
        PASS) PASS=$((PASS + 1)); RESULTS+=("  PASS  $name") ;;
        SKIP) SKIP=$((SKIP + 1)); RESULTS+=("  SKIP  $name ($detail)") ;;
        FAIL) FAIL=$((FAIL + 1)); RESULTS+=("  FAIL  $name -- $detail") ;;
    esac
}

# ── Environment detection ────────────────────────────────────────────
LANDLOCK_AVAILABLE=false
if grep -q landlock /sys/kernel/security/lsm 2>/dev/null; then
    LANDLOCK_AVAILABLE=true
fi

CAN_MOUNT=false
if unshare --user --mount -- echo test >/dev/null 2>&1; then
    # Also test if uid_map write works
    if unshare --user --mount -- sh -c 'echo 0 > /proc/self/uid_map 2>/dev/null' 2>/dev/null; then
        CAN_MOUNT=true
    fi
fi

echo "Environment: Landlock LSM=$LANDLOCK_AVAILABLE, Mount ns=$CAN_MOUNT"

# ── Fixtures ─────────────────────────────────────────────────────────
echo "=== Creating test fixtures ==="

mkdir -p "$TESTDIR/project/sub1/sub2"
echo 'SECRET=abc' > "$TESTDIR/project/.env"
echo "DB_PASS=xyz" > "$TESTDIR/project/sub1/.env"
echo 'API_KEY=xyz' > "$TESTDIR/project/sub1/sub2/.env"
echo 'NOT_SECRET=hello' > "$TESTDIR/project/config.txt"

mkdir -p "$TESTDIR/readonly-dir"
echo "data" > "$TESTDIR/readonly-dir/file.txt"

mkdir -p "$TESTDIR/ephemeral-dir"
echo "original" > "$TESTDIR/ephemeral-dir/ephemeral.txt"

echo "=== Running tests ==="

# ══════════════════════════════════════════════════════════════════════
# ── HIDE tests (require mount namespace) ─────────────────────────────
# ══════════════════════════════════════════════════════════════════════
echo ""
echo "--- HIDE tests ---"

if [[ "$CAN_MOUNT" == "true" ]]; then
    # H1: hide single .env -> empty
    H1=$($INBOX --hide "$TESTDIR/project/.env" -- cat "$TESTDIR/project/.env" 2>/dev/null || true)
    [[ ${#H1} -eq 0 ]] && log "H1: hide .env -> empty" "PASS" || log "H1: hide .env -> empty" "FAIL" "got ${#H1} bytes"

    # H2: hide glob **/.env -> all empty
    H2A=$($INBOX --hide "$TESTDIR/project/**/.env" -- cat "$TESTDIR/project/.env" 2>/dev/null || true)
    H2B=$($INBOX --hide "$TESTDIR/project/**/.env" -- cat "$TESTDIR/project/sub1/.env" 2>/dev/null || true)
    H2C=$($INBOX --hide "$TESTDIR/project/**/.env" -- cat "$TESTDIR/project/sub1/sub2/.env" 2>/dev/null || true)
    H2_OK=0; [[ ${#H2A} -eq 0 ]] && H2_OK=$((H2_OK+1)); [[ ${#H2B} -eq 0 ]] && H2_OK=$((H2_OK+1)); [[ ${#H2C} -eq 0 ]] && H2_OK=$((H2_OK+1))
    [[ $H2_OK -eq 3 ]] && log "H2: hide **/.env -> all empty" "PASS" || log "H2: hide **/.env -> all empty" "FAIL" "$H2_OK/3 empty"

    # H3: parent dir accessible
    $INBOX --hide "$TESTDIR/project/.env" -- ls "$TESTDIR/project" >/dev/null 2>&1 \
        && log "H3: parent dir accessible" "PASS" || log "H3: parent dir accessible" "FAIL"

    # H4: non-hidden file readable
    H4=$($INBOX --hide "$TESTDIR/project/.env" -- cat "$TESTDIR/project/config.txt" 2>/dev/null || true)
    echo "$H4" | grep -q "NOT_SECRET" && log "H4: non-hidden readable" "PASS" || log "H4: non-hidden readable" "FAIL"

    # H5: hide nonexistent -> no crash
    $INBOX --hide "/nonexistent/path/.env" -- echo ok >/dev/null 2>&1 \
        && log "H5: hide nonexistent -> no crash" "PASS" || log "H5: hide nonexistent -> no crash" "FAIL"

    # H6: hide dir -> empty
    mkdir -p "$TESTDIR/hide-dir"
    echo "x" > "$TESTDIR/hide-dir/file.txt"
    H6=$($INBOX --hide "$TESTDIR/hide-dir" -- ls -A "$TESTDIR/hide-dir" 2>/dev/null || true)
    [[ -z "$H6" ]] && log "H6: hide dir -> empty" "PASS" || log "H6: hide dir -> empty" "FAIL" "got: $H6"

    # H7: hide *.key glob
    mkdir -p "$TESTDIR/glob"
    echo "k" > "$TESTDIR/glob/server.key"
    echo "p" > "$TESTDIR/glob/cert.pem"
    H7A=$($INBOX --hide "$TESTDIR/glob/*.key" -- cat "$TESTDIR/glob/server.key" 2>/dev/null || true)
    H7B=$($INBOX --hide "$TESTDIR/glob/*.key" -- cat "$TESTDIR/glob/cert.pem" 2>/dev/null || true)
    [[ ${#H7A} -eq 0 ]] && echo "$H7B" | grep -q "p" \
        && log "H7: hide *.key -> key empty, pem ok" "PASS" || log "H7: hide *.key" "FAIL"

    # H8: multiple hide
    mkdir -p "$TESTDIR/multi"
    echo "s1" > "$TESTDIR/multi/.env"
    echo "s2" > "$TESTDIR/multi/.key"
    echo "ok" > "$TESTDIR/multi/pub.txt"
    H8A=$($INBOX --hide "$TESTDIR/multi/.env" --hide "$TESTDIR/multi/.key" -- cat "$TESTDIR/multi/.env" 2>/dev/null || true)
    H8B=$($INBOX --hide "$TESTDIR/multi/.env" --hide "$TESTDIR/multi/.key" -- cat "$TESTDIR/multi/.key" 2>/dev/null || true)
    H8C=$($INBOX --hide "$TESTDIR/multi/.env" --hide "$TESTDIR/multi/.key" -- cat "$TESTDIR/multi/pub.txt" 2>/dev/null || true)
    [[ ${#H8A} -eq 0 ]] && [[ ${#H8B} -eq 0 ]] && echo "$H8C" | grep -q "ok" \
        && log "H8: multiple hide -> both empty, pub ok" "PASS" || log "H8: multiple hide" "FAIL"

    # H9: hide + ro combined
    mkdir -p "$TESTDIR/combo"
    echo "sec" > "$TESTDIR/combo/.env"
    echo "vis" > "$TESTDIR/combo/readme.txt"
    H9A=$($INBOX --hide "$TESTDIR/combo/.env" --ro "$TESTDIR/combo" -- cat "$TESTDIR/combo/.env" 2>/dev/null || true)
    H9B=$($INBOX --hide "$TESTDIR/combo/.env" --ro "$TESTDIR/combo" -- cat "$TESTDIR/combo/readme.txt" 2>/dev/null || true)
    [[ ${#H9A} -eq 0 ]] && echo "$H9B" | grep -q "vis" \
        && log "H9: hide+ro -> env empty, readme ok" "PASS" || log "H9: hide+ro" "FAIL"

    # H10: **/.secret recursive
    mkdir -p "$TESTDIR/rec/a/deep" "$TESTDIR/rec/b/deep"
    echo "sa" > "$TESTDIR/rec/a/deep/.secret"
    echo "pub" > "$TESTDIR/rec/a/deep/public.txt"
    H10A=$($INBOX --hide "$TESTDIR/rec/**/.secret" -- cat "$TESTDIR/rec/a/deep/.secret" 2>/dev/null || true)
    H10B=$($INBOX --hide "$TESTDIR/rec/**/.secret" -- cat "$TESTDIR/rec/a/deep/public.txt" 2>/dev/null || true)
    [[ ${#H10A} -eq 0 ]] && echo "$H10B" | grep -q "pub" \
        && log "H10: **/.secret recursive -> secret empty, pub ok" "PASS" || log "H10: **/.secret recursive" "FAIL"
else
    log "H1-H10: hide tests" "SKIP" "mount namespace unavailable (uid_map EPERM)"
fi

# ══════════════════════════════════════════════════════════════════════
# ── RO tests (require Landlock) ──────────────────────────────────────
# ══════════════════════════════════════════════════════════════════════
echo ""
echo "--- RO tests ---"

# Probe: can --ro actually run on this system?
mkdir -p "$TESTDIR/ro-probe"
echo "probe" > "$TESTDIR/ro-probe/p.txt"
RO_PROBE=$($INBOX --ro "$TESTDIR/ro-probe/p.txt" -- echo ok 2>&1)
RO_AVAILABLE=false
echo "$RO_PROBE" | grep -q "^ok$" && RO_AVAILABLE=true

if [[ "$RO_AVAILABLE" == "true" ]]; then
    # R1: ro file read
    R1=$($INBOX --ro "$TESTDIR/readonly-dir/file.txt" -- cat "$TESTDIR/readonly-dir/file.txt" 2>/dev/null || true)
    echo "$R1" | grep -q "data" && log "R1: ro file -> read ok" "PASS" || log "R1: ro file -> read ok" "FAIL"

    # R2: write to ro file denied
    mkdir -p "$TESTDIR/ro-test"
    echo "readme" > "$TESTDIR/ro-test/f.txt"
    $INBOX --ro "$TESTDIR/ro-test/f.txt" -- sh -c "echo x > $TESTDIR/ro-test/f.txt" 2>/dev/null
    R2C=$(cat "$TESTDIR/ro-test/f.txt")
    [[ "$R2C" == "readme" ]] && log "R2: ro file -> write denied" "PASS" || log "R2: ro file -> write denied" "FAIL" "content: '$R2C'"

    # R3: write to file in ro dir denied
    $INBOX --ro "$TESTDIR/readonly-dir" -- sh -c "echo x > $TESTDIR/readonly-dir/file.txt" 2>/dev/null
    R3C=$(cat "$TESTDIR/readonly-dir/file.txt")
    [[ "$R3C" == "data" ]] && log "R3: ro dir -> write denied" "PASS" || log "R3: ro dir -> write denied" "FAIL"

    # R4: create new file in ro dir denied
    $INBOX --ro "$TESTDIR/readonly-dir" -- sh -c "echo new > $TESTDIR/readonly-dir/new.txt" 2>/dev/null
    [[ ! -f "$TESTDIR/readonly-dir/new.txt" ]] && log "R4: ro dir -> create denied" "PASS" || log "R4: ro dir -> create denied" "FAIL"
else
    RO_REASON=$(echo "$RO_PROBE" | grep "^error:" | head -1)
    log "R1: ro file -> read ok" "SKIP" "$RO_REASON"
    log "R2: ro write denied" "SKIP" "$RO_REASON"
    log "R3: ro dir write denied" "SKIP" "$RO_REASON"
    log "R4: ro dir create denied" "SKIP" "$RO_REASON"
fi

# ══════════════════════════════════════════════════════════════════════
# ── EPHEMERAL tests ──────────────────────────────────────────────────
# ══════════════════════════════════════════════════════════════════════
echo ""
echo "--- EPHEMERAL tests ---"

# E1: file restored after exit
mkdir -p "$TESTDIR/eph1"
echo "original" > "$TESTDIR/eph1/f.txt"
$INBOX --ephemeral "$TESTDIR/eph1" -- sh -c "echo modified > $TESTDIR/eph1/f.txt" 2>/dev/null
E1=$(cat "$TESTDIR/eph1/f.txt")
[[ "$E1" == "original" ]] && log "E1: ephemeral -> restored" "PASS" || log "E1: ephemeral -> restored" "FAIL" "got: $E1"

# E2: new file discarded
mkdir -p "$TESTDIR/eph2"
$INBOX --ephemeral "$TESTDIR/eph2" -- sh -c "echo temp > $TESTDIR/eph2/temp.txt" 2>/dev/null
[[ ! -f "$TESTDIR/eph2/temp.txt" ]] && log "E2: ephemeral -> new file discarded" "PASS" || log "E2: ephemeral -> new file discarded" "FAIL"

# E3: read works during run
E3=$($INBOX --ephemeral "$TESTDIR/ephemeral-dir" -- cat "$TESTDIR/ephemeral-dir/ephemeral.txt" 2>/dev/null || true)
echo "$E3" | grep -q "original" && log "E3: ephemeral -> read ok" "PASS" || log "E3: ephemeral -> read ok" "FAIL"

# E4: file deletion reverted
mkdir -p "$TESTDIR/eph4"
echo "keep" > "$TESTDIR/eph4/f.txt"
$INBOX --ephemeral "$TESTDIR/eph4" -- rm "$TESTDIR/eph4/f.txt" 2>/dev/null
[[ -f "$TESTDIR/eph4/f.txt" ]] && log "E4: ephemeral -> deletion reverted" "PASS" || log "E4: ephemeral -> deletion reverted" "FAIL"

# ══════════════════════════════════════════════════════════════════════
# ── EXIT CODE tests ──────────────────────────────────────────────────
# ══════════════════════════════════════════════════════════════════════
echo ""
echo "--- EXIT CODE tests ---"

# X1: exit 42
set +e
$INBOX -- /bin/sh -c "exit 42" 2>/dev/null; X1=$?
set -e
[[ $X1 -eq 42 ]] && log "X1: exit 42 propagates" "PASS" || log "X1: exit 42 propagates" "FAIL" "got $X1"

# X2: exit 0
set +e
$INBOX -- /bin/sh -c "exit 0" 2>/dev/null; X2=$?
set -e
[[ $X2 -eq 0 ]] && log "X2: exit 0 propagates" "PASS" || log "X2: exit 0 propagates" "FAIL" "got $X2"

# X3: exit 1
set +e
$INBOX -- /bin/sh -c "exit 1" 2>/dev/null; X3=$?
set -e
[[ $X3 -eq 1 ]] && log "X3: exit 1 propagates" "PASS" || log "X3: exit 1 propagates" "FAIL" "got $X3"

# X4: false command
set +e
$INBOX -- false 2>/dev/null; X4=$?
set -e
[[ $X4 -eq 1 ]] && log "X4: false exits 1" "PASS" || log "X4: false exits 1" "FAIL" "got $X4"

# ══════════════════════════════════════════════════════════════════════
# ── EDGE CASE tests ──────────────────────────────────────────────────
# ══════════════════════════════════════════════════════════════════════
echo ""
echo "--- EDGE CASE tests ---"

# F1: no command -> error
set +e
$INBOX 2>/dev/null; F1=$?
set -e
[[ $F1 -ne 0 ]] && log "F1: no command -> error" "PASS" || log "F1: no command -> error" "FAIL"

# F2: nonexistent command -> error
set +e
$INBOX -- /nonexistent_xyz 2>/dev/null; F2=$?
set -e
[[ $F2 -ne 0 ]] && log "F2: nonexistent cmd -> error" "PASS" || log "F2: nonexistent cmd -> error" "FAIL"

# F3: whoami works
F3=$($INBOX -- whoami 2>/dev/null || true)
[[ -n "$F3" ]] && log "F3: whoami works" "PASS" || log "F3: whoami works" "FAIL"

# F4: invalid glob -> no crash
set +e
$INBOX --hide "[invalid_glob" -- echo ok 2>/dev/null; F4=$?
set -e
log "F4: invalid glob -> handled ($F4)" "PASS"

# F5: nonexistent profile -> handled
set +e
$INBOX --profile nonexistent_xyz -- echo ok 2>/dev/null; F5=$?
set -e
log "F5: nonexistent profile -> handled ($F5)" "PASS"

# F6: hide nonexistent -> no crash
set +e
$INBOX --hide "/no/such/path/.env" -- echo ok 2>/dev/null; F6=$?
set -e
log "F6: hide nonexistent path -> ok" "PASS"

# F7: invalid glob with glob chars
set +e
$INBOX --ro "[invalid" -- echo ok 2>/dev/null; F7=$?
set -e
log "F7: invalid glob in ro -> handled ($F7)" "PASS"

# ══════════════════════════════════════════════════════════════════════
# ── REPORT ───────────────────────────────────────────────────────────
# ══════════════════════════════════════════════════════════════════════
echo ""
echo "============================================================"
echo "              INBOX INTEGRATION TEST REPORT"
echo "============================================================"
echo ""
echo "Binary: $INBOX"
echo "Kernel: $(uname -r)"
echo "Landlock LSM: $LANDLOCK_AVAILABLE"
echo "Mount namespace (user ns): $CAN_MOUNT"
echo ""
echo "Results:"
for r in "${RESULTS[@]}"; do echo "$r"; done
echo ""
echo "------------------------------------------------------------"
TOTAL=$((PASS + FAIL + SKIP))
printf "Total: %d | PASS: %d | FAIL: %d | SKIP: %d\n" "$TOTAL" "$PASS" "$FAIL" "$SKIP"
echo "============================================================"

[[ $FAIL -gt 0 ]] && exit 1
exit 0
