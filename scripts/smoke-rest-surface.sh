#!/usr/bin/env bash
# scripts/smoke-rest-surface.sh — the 41-row REST live smoke (issues 24+29 wave, 2026-11-08).
#
# Modes:
#   SMOKE_SELFTEST=1 scripts/smoke-rest-surface.sh
#       Canned-derivation selftest (the wave's one RED→GREEN unit). Asserts the
#       verdict derivation on fixed (status, body, expectation) cases including
#       the deliberately-wrong control (expect 418 for a 200 answer → must FAIL).
#   scripts/smoke-rest-surface.sh
#       Full run: scratch engine spawn, seed actions, the pre-registered 41-row
#       walk (stop-on-mismatch), the token-less auth walk (38 gated rows), the
#       D-11 wildcard sub-case, rows.tsv + rest-run.log into EVIDENCE_DIR.
#   scripts/smoke-rest-surface.sh --live-base-url http://127.0.0.1:3742 \
#       --live-token-file <root>/.auth-token --evidence-dir <dir> [--live-project <id>]
#       GET-only read-only pass against a running engine. Refuses every non-GET
#       method and self-checks its own log; the run is invalid if any non-GET
#       was issued.
#
# The expectation table below is PRE-REGISTERED (blueprint
# blueprint-20261108-issues-24-29-smoke-r2, re-pinned from handler source).
# A row whose live answer contradicts its expectation is a STOP condition:
# the mismatch is reported with the raw response and the expectation is never
# adjusted mid-run to fit.
#
# Token hygiene: tokens are read from files, sent in headers only, and never
# printed (live token: length + 4-char prefix in the log, nothing else).
set -u -o pipefail

# --------------------------------------------------------------------------
# The pre-registered table: num|method|path|statuses|sub|class
#   statuses: the accepted status set ("200" single-valued for all 41 rows)
#   sub: the body substring an ASSERTED row must carry; "-" = none
#   {probe-a}/{probe-b} are runtime placeholders resolved from seed responses.
# --------------------------------------------------------------------------
load_table() {
	TABLE=$(cat <<'ROWS'
1|GET|/api/health|200|"success":true|ASSERTED
2|GET|/api/tags|200|"success":true|ASSERTED
3|GET|/api/memories|200|"success":true|ASSERTED
4|POST|/api/memories|200|"id":"mem_|ASSERTED
5|DELETE|/api/memories/{probe-a}|200|"success":true|ASSERTED
6|PUT|/api/memories/{probe-b}|200|"success":true|ASSERTED
7|POST|/api/memories/bulk-delete|200|"deleted":0|ASSERTED
8|PUT|/api/memories/bulk-delete|200|Memory not found|ASSERTED
9|GET|/api/search?q=probe|200|"success":true|ASSERTED
10|GET|/api/stats|200|"success":true|ASSERTED
11|POST|/api/memories/{probe-b}/pin|200|"success":true|ASSERTED
12|POST|/api/memories/{probe-b}/unpin|200|"success":true|ASSERTED
13|POST|/api/cleanup|501|cleanup is not available in this build|ASSERTED
14|POST|/api/deduplicate|200|exactDuplicatesDeleted|ASSERTED
15|GET|/api/migration/detect|501|the endpoint is not available in this build|ASSERTED
16|GET|/api/migration/tags/detect|501|the endpoint is not available in this build|ASSERTED
17|POST|/api/migration/tags/run-batch|501|the endpoint is not available in this build|ASSERTED
18|GET|/api/migration/tags/progress|501|the endpoint is not available in this build|ASSERTED
19|POST|/api/migration/run|501|the endpoint is not available in this build|ASSERTED
20|DELETE|/api/prompts/smoke-unknown|200|"success":|ASSERTED
21|POST|/api/prompts/bulk-delete|200|ids array is required|ASSERTED
22|GET|/api/user-profile|200|"success":true|ASSERTED
23|GET|/api/user-profile/changelog?profileId=probe|200|"success":true|ASSERTED
24|GET|/api/user-profile/snapshot?chlogId=probe|200|Changelog not found|ASSERTED
25|POST|/api/user-profile/refresh|200|"profileExists":false|ASSERTED
26|POST|/api/user-profile/ai-cleanup|200|No profile found to clean up|ASSERTED
27|POST|/api/user-profile/ai-cleanup/apply|200|No pending cleanup found. Run AI cleanup first.|ASSERTED
28|PATCH|/api/user-profile/item|200|No profile found|ASSERTED
29|GET|/api/skills|200|"success":true|ASSERTED
30|POST|/api/skills|200|"success":true|ASSERTED
31|GET|/api/skills/ship-it|200|ship-it|ASSERTED
32|PUT|/api/skills/ship-it|200|"success":true|ASSERTED
33|DELETE|/api/skills/ship-it|200|"success":true|ASSERTED
34|GET|/skills/index.json|200|skills|ASSERTED
35|GET|/skills/ship-it/ship-it.md|200|## Ship|ASSERTED
36|GET|/api/projects/smoke-proj/sessions|200|"data":[]|ASSERTED
37|DELETE|/api/projects/smoke-proj/sessions/smoke-unknown|404|Session not found|ASSERTED
38|GET|/api/projects/smoke-proj/milestones|200|"data":[]|ASSERTED
39|GET|/api/user-profiles|200|"success":true|ASSERTED
40|GET|/api/projects|200|"success":true|ASSERTED
41|DELETE|/api/user-profile|200|"success":|ASSERTED
ROWS
)$'\n'
	# Walk order: rows in numeric order EXCEPT row 33 (the ship-it DELETE) runs
	# after the catalog rows 34/35 so the delete exercises a live entry.
	WALK_ORDER="1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 34 35 33 36 37 38 39 40 41"
	# The token-gated rows: all but 1 (health) and 34/35 (loopback-public catalog).
	AUTH_EXEMPT=" 1 34 35 "
	AUTH_WALK_WIDTH=$(( $(printf '%s\n' "$TABLE" | grep -c .) - 3 ))
}

# table_row <num> -> prints "method|path|statuses|sub|class"
table_row() {
	local num=$1 line
	line=$(printf '%s' "$TABLE" | grep -E "^${num}\|" || true)
	[ -n "$line" ] || return 1
	printf '%s' "${line#*|}"
}

# --------------------------------------------------------------------------
# The verdict derivation (the wave's RED→GREEN unit).
#   derive_verdict <class> <statuses> <sub> <actual_status> <actual_body>
# Prints exactly one verdict word on stdout:
#   ASSERTED — class ASSERTED, actual status is in the set AND the body
#              carries the expected substring (status alone is never enough);
#              with sub "-", the status comparison is the whole assertion.
#   MEASURED — class MEASURED and the actual status is in the set.
#   BLOCKED  — class BLOCKED (truth recording; the unblocker is the note).
#   FAIL     — any other outcome (wrong status, missing substring, class that
#              cannot occur). The walker turns a FAIL into a STOP.
# --------------------------------------------------------------------------
# GREEN-LANDMARK: derive_verdict is implemented here (see Step 3 ledger).
derive_verdict() {
	# derive_verdict <class> <statuses> <sub> <actual_status> <actual_body>
	local class=$1 statuses=$2 sub=$3 astatus=$4 abody=$5
	# BLOCKED records truth; it never compares a live answer.
	if [ "$class" = "BLOCKED" ]; then
		printf 'BLOCKED'
		return 0
	fi
	# The actual status must be in the accepted set (single-valued today).
	local status_ok=0 s
	for s in $(printf '%s' "$statuses" | tr ',' ' '); do
		[ "$astatus" = "$s" ] && status_ok=1
	done
	if [ "$status_ok" -ne 1 ]; then
		printf 'FAIL'
		return 0
	fi
	# MEASURED: status alone (the sub slot is "-" by construction).
	if [ "$class" = "MEASURED" ]; then
		printf 'MEASURED'
		return 0
	fi
	# ASSERTED: the body must also carry the expected substring; with sub "-",
	# the status comparison is the whole assertion.
	if [ "$class" = "ASSERTED" ]; then
		if [ "$sub" = "-" ] || printf '%s' "$abody" | grep -qF -- "$sub"; then
			printf 'ASSERTED'
		else
			printf 'FAIL'
		fi
		return 0
	fi
	printf 'FAIL'
	return 0
}

# --------------------------------------------------------------------------
# Selftest
# --------------------------------------------------------------------------
selftest() {
	load_table
	local rc=0
	check_case() {
		local name=$1 expected=$2 class=$3 statuses=$4 sub=$5 astatus=$6 abody=$7 got
		got=$(derive_verdict "$class" "$statuses" "$sub" "$astatus" "$abody" 2>/dev/null)
		if [ "$got" = "$expected" ]; then
			printf 'ok   %s -> %s\n' "$name" "$got"
		else
			printf 'FAIL %s: expected verdict %s, got %s\n' "$name" "$expected" "${got:-<none>}"
			rc=1
		fi
	}
	# 1. happy ASSERTED: status + substring both match
	check_case "asserted-ok" "ASSERTED" "ASSERTED" "200" '"success":true' 200 '{"success":true,"data":[]}'
	# 2. THE DELIBERATELY-WRONG CONTROL: expect 418 for a 200 answer -> FAIL.
	#    A derivation that marks every row PASS without comparing status/body
	#    fails this case and the selftest exits non-zero.
	check_case "wrong-expectation-control" "FAIL" "ASSERTED" "418" '"success":true' 200 '{"success":true}'
	# 3. status match but substring missing -> FAIL (the anti-toothless body arm)
	check_case "asserted-body-mismatch" "FAIL" "ASSERTED" "200" '"deleted":0' 200 '{"success":true,"data":{"deleted":3}}'
	# 4. wrong status with right substring -> FAIL
	check_case "asserted-status-mismatch" "FAIL" "ASSERTED" "501" 'the endpoint is not available in this build' 200 '{"error":"the endpoint is not available in this build"}'
	# 5. MEASURED: status alone
	check_case "measured-ok" "MEASURED" "MEASURED" "200" "-" 200 '{"anything":1}'
	check_case "measured-status-only" "FAIL" "MEASURED" "200" "-" 500 'server error'
	# 6. BLOCKED: recorded as truth, never asserted against a live answer
	check_case "blocked-truth" "BLOCKED" "BLOCKED" "501" "-" 999 ''
	# 7. unknown class cannot pass
	check_case "unknown-class" "FAIL" "SOMEOTHER" "200" "-" 200 '{}'
	# 8. sub "-" with matching status asserts on status alone
	check_case "asserted-no-sub" "ASSERTED" "ASSERTED" "200" "-" 200 'whatever body'
	if [ "$rc" -eq 0 ]; then
		printf 'SMOKE_SELFTEST: PASS (9 cases incl. the wrong-expectation control)\n'
	else
		printf 'SMOKE_SELFTEST: FAIL\n' >&2
	fi
	return "$rc"
}

# --------------------------------------------------------------------------
# Small helpers
# --------------------------------------------------------------------------
log() { printf '%s %s\n' "$(date -u '+%H:%M:%S')" "$*" | tee -a "$RUNLOG"; }
die() { printf 'SMOKE-STOP: %s\n' "$*" >&2; log "STOP: $*"; exit "${2:-2}"; }

free_port() {
	node -e 'const s=require("net").createServer();s.listen(0,"127.0.0.1",()=>{console.log(s.address().port);s.close()})'
}

json_field() {
	# json_field <file-with-json> <js-expr over obj>  (node is in the toolchain)
	node -e 'let s="";process.stdin.on("data",c=>s+=c).on("end",()=>{try{const o=JSON.parse(s);const v=('"$2"');process.stdout.write(v===undefined?"":String(v))}catch(e){process.exit(3)}})' <"$1"
}

# http <method> <url> [body] [token] -> sets HTTP_CODE and HTTP_BODY
HTTP_CODE=""; HTTP_BODY=""
http() {
	local method=$1 url=$2 body=${3:-} token=${4:-} args=() tmp
	tmp=$(mktemp)
	args=(-sS --max-time 10 -o "$tmp" -w '%{http_code}' -X "$method")
	[ -n "$body" ] && args+=(-H 'content-type: application/json' --data "$body")
	[ -n "$token" ] && args+=(-H "x-opencode-mem-token: $token")
	HTTP_CODE=$(curl "${args[@]}" "$url" 2>"$CURLLOG") || HTTP_CODE="000"
	HTTP_CODE=$(printf '%s' "$HTTP_CODE" | tr -d '\r\n')
	HTTP_BODY=$(tr '\t\r\n' '   ' <"$tmp")
	rm -f "$tmp"
}

# --------------------------------------------------------------------------
# Full-run mode
# --------------------------------------------------------------------------
ENGINE=""
D11_ENGINE=""
MAIN_FD9=""
D11_FD9=""
ROOT=""
D11_ROOT=""

teardown() {
	local rc=$?
	if [ -n "$D11_ENGINE" ]; then
		kill -TERM "$D11_ENGINE" 2>/dev/null || true
		( sleep 10; kill -KILL "$D11_ENGINE" 2>/dev/null ) & local wd2=$!
		wait "$D11_ENGINE" 2>/dev/null || true
		kill "$wd2" 2>/dev/null || true
		wait "$wd2" 2>/dev/null || true
		D11_ENGINE=""
		eval "exec $D11_FD9>&-" 2>/dev/null || true
	fi
	if [ -n "$ENGINE" ]; then
		kill -TERM "$ENGINE" 2>/dev/null || true
		( sleep 30; kill -KILL "$ENGINE" 2>/dev/null ) & local wd=$!
		wait "$ENGINE" 2>/dev/null || true
		kill "$wd" 2>/dev/null || true
		wait "$wd" 2>/dev/null || true
		ENGINE=""
	fi
	[ -n "$MAIN_FD9" ] && eval "exec $MAIN_FD9>&-" 2>/dev/null || true
	log "teardown complete (rc=$rc)"
	exit "$rc"
}

spawn_engine() {
	# spawn_engine <bin> <root> <host> <port> <dist> [fdvar] — sets ENGINE_PID.
	# fdvar (optional) names a variable to record the guard-fd number in, so a
	# second concurrent engine's guard does not close the first one's fd 9.
	local bin=$1 root=$2 host=$3 port=$4 dist=$5 fdvar=${6:-}
	mkfifo "$root/stdin.fifo"
	local fd
	exec {fd}<>"$root/stdin.fifo"
	[ -n "$fdvar" ] && printf -v "$fdvar" '%s' "$fd"
	(
		mkdir -p "$root/home"
		exec env HOME="$root/home" OPENCODE_MEM_LOG_FILE="$root/engine.log" \
			"$bin" --mode=standalone --storage-root "$root" --web-host "$host" \
			--web-port "$port" --web-dist "$dist" <"$root/stdin.fifo" \
			>"$root/server.stdout.log" 2>"$root/server.stderr.log"
	) &
	ENGINE_PID=$!
}

wait_health() {
	# wait_health <port> — bounded; answers 0 iff /api/health is 200
	local port=$1 code=000
	for _ in $(seq 1 120); do
		code=$(curl --max-time 3 -sS -o /dev/null -w '%{http_code}' \
			"http://127.0.0.1:$port/api/health" 2>/dev/null) || code=000
		[ "$code" = "200" ] && return 0
		sleep 0.5
	done
	printf 'last health code %s\n' "$code" >&2
	return 1
}

full_run() {
	load_table
	local bin=$1 dist=$2 evidence=$3
	local stamp port
	stamp=$(date -u '+%Y%m%dT%H%M%SZ')
	SCROOT="${SMOKE_SCRATCH_ROOT:-$(pwd)/.tmp/scratch/smoke-24-29/$stamp}"
	ROOT="$SCROOT/root"
	RUNLOG="$evidence/rest-run.log"
	CURLLOG="$SCROOT/curl.err"
	mkdir -p "$ROOT/home" "$evidence" "$SCROOT"
	exec 2> >(tee -a "$RUNLOG" >&2)
	: >"$RUNLOG"
	trap teardown EXIT INT TERM
	log "engine binary: $bin"
	log "web dist: $dist"
	log "scratch root: $ROOT"

	# Model cache: hardlink copy (fallback recursive copy) from the host cache.
	#
	# LOCATION (measured 2026-11-08, r2 Step-5 stop diagnosis): the engine's
	# embedder resolves its cache at the RESOLVED CONFIG's storage path —
	# `$HOME/.opencode-mem/data/.cache` (engine.rs:1548 embedder_from_config
	# over core_config; composition.rs:419 local_cache_root =
	# config.storage_path.join(".cache"); config/mod.rs:342 storage_path =
	# expand_path("~/.opencode-mem/data", home)). The CLI --storage-root
	# overrides only the STORE's path (main.rs:188), never the embedder's.
	# The cache therefore goes under the sandbox HOME's data dir, which is
	# also exactly where a real deployment keeps it (why the historical
	# real-HOME recipe worked). A copy at <root>/.cache is NOT read.
	if [ -d "$HOME/.opencode-mem/data/.cache/Xenova" ]; then
		mkdir -p "$ROOT/home/.opencode-mem/data/.cache"
		cp -al "$HOME/.opencode-mem/data/.cache/Xenova" "$ROOT/home/.opencode-mem/data/.cache/Xenova" 2>/dev/null ||
			cp -r "$HOME/.opencode-mem/data/.cache/Xenova" "$ROOT/home/.opencode-mem/data/.cache/Xenova"
		log "model cache provisioned into $ROOT/home/.opencode-mem/data/.cache/Xenova (resolved-config location)"
	else
		log "host model cache NOT found at ~/.opencode-mem/data/.cache/Xenova - embed-backed rows will fail their contract"
	fi

	port=$(free_port)
	log "spawning scratch engine on 127.0.0.1:$port"
	spawn_engine "$bin" "$ROOT" "127.0.0.1" "$port" "$dist" "MAIN_FD9"
	ENGINE=$ENGINE_PID
	wait_health "$port" || die "scratch engine did not answer 200 on /api/health (bounded wait)"
	log "scratch engine healthy on port $port"
	BASE="http://127.0.0.1:$port"

	# Token mint: one token-less token-gated request (401) materialises the file.
	http GET "$BASE/api/stats"
	[ "$HTTP_CODE" = "401" ] || die "token-mint probe /api/stats answered $HTTP_CODE, expected 401"
	TOKEN=$(tr -d '\r\n' <"$ROOT/.auth-token")
	printf '%s' "$TOKEN" | grep -qE '^[0-9a-f]{64}$' || die "token file is not 64 hex chars"
	log "token minted (64 hex, never printed)"

	# Seed constants (canonical tags from crates/mem-web/tests/shard_model_authority.rs:33-35)
	USER_TAG="opencode_user_c1c2c3d4e5f67890"
	PROBE_A_BODY="{\"content\":\"smoke probe-a row 4 (issues-24-29 wave)\",\"containerTag\":\"$USER_TAG\",\"tags\":[\"smoke-wave-24-29\"]}"
	PROBE_B_BODY="{\"content\":\"smoke probe-b seeded lifecycle, row 9 searches the term probe\",\"containerTag\":\"$USER_TAG\",\"tags\":[\"smoke-wave-24-29\"]}"
	SHIP_IT_BODY='{"id":"ship-it","name":"Ship It","description":"Wave smoke ship-it skill","content":"## Ship\n\nShip the wave."}'
	PUT_SKILL_BODY='{"description":"Wave smoke ship-it skill (updated row 32)"}'
	SEED_JSON="$SCROOT/seed.json"
	PROBE_A_ID=""; PROBE_B_ID=""

	# rows.tsv: verdicts in field 6.
	ROWS_TSV="$evidence/rows.tsv"
	: >"$ROWS_TSV"

	# run_row <num> — resolves placeholders, sends the request, derives the verdict
	run_row() {
		local num=$1 fields method path statuses sub class
		fields=$(table_row "$num") || die "row $num missing from the table"
		IFS='|' read -r method path statuses sub class <<EOF
$fields
EOF
		# Row bodies (pre-registered; row 28's body omits type and action per the gate fix).
		local body=""
		case $num in
			4) body="$PROBE_A_BODY" ;;
			7) body='{"ids":["probe-c"]}' ;;
			17) body='{"batchSize":5}' ;;
			19) body='{"strategy":"fresh-start"}' ;;
			21) body='{"ids":[]}' ;;
			28) body='{"index":0}' ;;
			30) body="$SHIP_IT_BODY" ;;
			32) body="$PUT_SKILL_BODY" ;;
		esac
		path=${path//\{probe-a\}/$PROBE_A_ID}
		path=${path//\{probe-b\}/$PROBE_B_ID}
		log "row $num: $method $path (expect $statuses / $sub)"
		http "$method" "$BASE$path" "$body" "$TOKEN"
		local verdict
		verdict=$(derive_verdict "$class" "$statuses" "$sub" "$HTTP_CODE" "$HTTP_BODY")
		local excerpt=${HTTP_BODY:0:160}
		printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
			"$num" "$method" "$path" "$statuses" "$sub" "$verdict" "$HTTP_CODE" "$excerpt" >>"$ROWS_TSV"
		log "row $num: verdict=$verdict code=$HTTP_CODE"
		# Side effects the walk depends on.
		if [ "$num" = "4" ]; then
			printf '%s' "$HTTP_BODY" >"$SEED_JSON"
			PROBE_A_ID=$(json_field "$SEED_JSON" 'o.data.id||o.id||""')
			[ -n "$PROBE_A_ID" ] || die "row 4 POST did not yield a memory id (body: $HTTP_BODY)"
		fi
		# STOP-ON-MISMATCH: a failed row stops the run with the raw response.
		if [ "$verdict" = "FAIL" ]; then
			printf '%s\n' "row $num raw response (code $HTTP_CODE):" >>"$RUNLOG"
			printf '%s\n' "$HTTP_BODY" >>"$RUNLOG"
			die "row $num FAILED its pre-registered expectation (see rest-run.log for the raw response)"
		fi
	}

	# seed_probe_b — SEED ACTION, not a row. Failure is a harness-precondition STOP.
	seed_probe_b() {
		log "seed action: probe-b POST (between rows 5 and 6)"
		http POST "$BASE/api/memories" "$PROBE_B_BODY" "$TOKEN"
		if [ "$HTTP_CODE" != "200" ]; then
			printf '%s\n' "probe-b seed raw response (code $HTTP_CODE): $HTTP_BODY" >>"$RUNLOG"
			die "seed action probe-b POST failed (code $HTTP_CODE) - rows gated on it go undispositioned"
		fi
		printf '%s' "$HTTP_BODY" >"$SEED_JSON"
		PROBE_B_ID=$(json_field "$SEED_JSON" 'o.data.id||o.id||""')
		[ -n "$PROBE_B_ID" ] || die "seed action probe-b POST returned no id (body: $HTTP_BODY)"
		log "seed action: probe-b id minted"
	}

	# teardown_probe_b — hygiene DELETE after row 12 (seed action).
	teardown_probe_b() {
		log "seed action: probe-b teardown DELETE (after row 12)"
		http DELETE "$BASE/api/memories/$PROBE_B_ID" "" "$TOKEN"
		if [ "$HTTP_CODE" != "200" ]; then
			printf '%s\n' "probe-b teardown raw response (code $HTTP_CODE): $HTTP_BODY" >>"$RUNLOG"
			die "seed action probe-b teardown DELETE failed (code $HTTP_CODE)"
		fi
		log "seed action: probe-b torn down"
	}

	local num
	for num in $WALK_ORDER; do
		run_row "$num"
		[ "$num" = "5" ] && seed_probe_b
		[ "$num" = "12" ] && teardown_probe_b
	done

	# The auth walk: every token-gated row answers 401 without a token.
	log "auth walk: $AUTH_WALK_WIDTH token-gated rows, token-less"
	local walked=0
	for num in $WALK_ORDER; do
		case "$AUTH_EXEMPT" in
			*" $num "*) continue ;;
		esac
		local fields method path
		fields=$(table_row "$num")
		IFS='|' read -r method path _rest <<EOF
$fields
EOF
		path=${path%%\?*}
		path=${path//\{probe-a\}/authwalk}
		path=${path//\{probe-b\}/authwalk}
		http "$method" "$BASE$path" "" ""
		if [ "$HTTP_CODE" != "401" ]; then
			printf '%s\n' "auth-walk row $num ($method $path) answered $HTTP_CODE token-less: $HTTP_BODY" >>"$RUNLOG"
			die "auth walk FAILED: row $num answered $HTTP_CODE without a token (expected 401)"
		fi
		walked=$((walked + 1))
	done
	log "auth walk: $walked/$AUTH_WALK_WIDTH gated rows answered 401"
	[ "$walked" -eq "$AUTH_WALK_WIDTH" ] || die "auth walk width mismatch: walked $walked, expected $AUTH_WALK_WIDTH"

	# D-11 sub-case. DERIVATION CORRECTION (2026-11-08, measured): the
	# pre-registered shape ("a second engine on 0.0.0.0 answers 401
	# token-less") is not realizable through the production launcher —
	# assert_network_auth (crates/mem-web/src/api_auth.rs:30-46, called from
	# server.rs:96) REFUSES a non-loopback bind unless an API token or Basic
	# auth is configured, and the root launcher hardcodes api_token: None
	# (src/main.rs:176; the config's webServerApiToken has no consumer). The
	# engine therefore fail-closes the launch itself: no socket, structured
	# web.launch_failed on stderr. That refusal is the live wildcard-half
	# evidence — an "always-public" mutant would launch and serve — and the
	# exact 401-token-less / 200-with-token contract on a wildcard bind is
	# pinned at the mem-web router seam
	# (crates/mem-web/tests/api_route_auth.rs:510-540, workspace suite).
	log "D-11: spawning second engine on --web-host 0.0.0.0 (throwaway root, seconds only)"
	D11_ROOT="$SCROOT/d11-root"
	mkdir -p "$D11_ROOT/home"
	local d11port
	d11port=$(free_port)
	spawn_engine "$bin" "$D11_ROOT" "0.0.0.0" "$d11port" "$dist" "D11_FD9"
	D11_ENGINE=$ENGINE_PID
	local d11code=000
	for _ in $(seq 1 20); do
		d11code=$(curl --max-time 2 -sS -o /dev/null -w '%{http_code}' \
			"http://127.0.0.1:$d11port/skills/index.json" 2>/dev/null) || d11code=000
		[ "$d11code" != "000" ] && break
		if ! kill -0 "$D11_ENGINE" 2>/dev/null; then
			break
		fi
		sleep 0.5
	done
	if [ "$d11code" != "000" ]; then
		printf '%s\n' "D-11 wildcard engine ANSWERED $d11code on /skills/index.json - the launch-refusal posture is gone" >>"$RUNLOG"
		die "D-11 sub-case FAILED: the 0.0.0.0 engine served a request (code $d11code); expected a launch refusal (no socket)"
	fi
	if grep -q '"event":"web.launch_failed"' "$D11_ROOT/server.stderr.log" 2>/dev/null; then
		log "D-11: wildcard bind REFUSED the web launch (web.launch_failed, no socket) - fail-closed as derived; 401/200 contract pinned at api_route_auth.rs:510-540"
	else
		printf '%s\n' "D-11 wildcard engine produced neither a served request nor web.launch_failed" >>"$RUNLOG"
		die "D-11 sub-case FAILED: no launch refusal observed (no web.launch_failed in $D11_ROOT/server.stderr.log)"
	fi

	# Close-out: recompute the class split from rows.tsv (field 6), never from prose.
	local missing total asserted measured blocked failed
	total=$(awk -F'\t' 'END{print NR}' "$ROWS_TSV")
	missing=$((41 - total))
	asserted=$(awk -F'\t' '$6=="ASSERTED"' "$ROWS_TSV" | wc -l)
	measured=$(awk -F'\t' '$6=="MEASURED"' "$ROWS_TSV" | wc -l)
	blocked=$(awk -F'\t' '$6=="BLOCKED"' "$ROWS_TSV" | wc -l)
	failed=$(awk -F'\t' '$6=="FAIL"' "$ROWS_TSV" | wc -l)
	log "class split (awk recount of rows.tsv): total=$total ASSERTED=$asserted MEASURED=$measured BLOCKED=$blocked FAIL=$failed MISSING=$missing"
	if [ "$total" -ne 41 ] || [ "$failed" -ne 0 ] || [ "$missing" -ne 0 ]; then
		die "close-out check failed: total=$total FAIL=$failed MISSING=$missing"
	fi
	log "SMOKE RUN COMPLETE: 41/41 dispositioned, 0 FAIL, 0 MISSING, 0 seed failures"
}

# --------------------------------------------------------------------------
# Live read-only mode (GET-only, never mutates the live store)
# --------------------------------------------------------------------------
LIVECALLS=0
live_get() {
	# live_get <method>|<path> — GET only; any other method is a harness bug and refuses.
	local spec=$1 method path
	method=$(printf '%s' "$spec" | cut -d'|' -f1)
	path=$(printf '%s' "$spec" | cut -d'|' -f2-)
	[ "$method" = "GET" ] || die "live mode refuses non-GET ($method)"
	printf 'LIVECALL GET %s\n' "$path" >>"$RUNLOG"
	http GET "$LIVE_BASE$path" "" "$LIVE_TOKEN"
	LIVECALLS=$((LIVECALLS + 1))
	printf 'GET %s -> %s %s\n' "$path" "$HTTP_CODE" "${HTTP_BODY:0:200}" >>"$RUNLOG"
}

live_run() {
	load_table
	local base=$1 tokenfile=$2 evidence=$3 projid=${4:-}
	LIVE_BASE=$base
	RUNLOG="$evidence/live-readonly-pass.log"
	CURLLOG="$evidence/live-curl.err"
	LIVE_RC=0
	: >"$RUNLOG"
	trap live_selfcheck EXIT INT TERM
	if [ ! -f "$tokenfile" ]; then
		LIVE_RC=2
		die "live token file not found: $tokenfile"
	fi
	LIVE_TOKEN=$(tr -d '\r\n' <"$tokenfile")
	if ! printf '%s' "$LIVE_TOKEN" | grep -qE '^[0-9a-f]{64}$'; then
		LIVE_RC=2
		die "live token file is not 64 hex chars"
	fi
	log "live pass against $LIVE_BASE (token len=${#LIVE_TOKEN} prefix=${LIVE_TOKEN:0:4}…, never printed)"
	log "GET-only mode: every call is a GET; the log self-check at exit enforces it"

	local report="$evidence/live-readonly-pass.md"
	: >"$report"
	{
		printf '# Live 3742 GET-only read-only pass (issues 24+29 wave, %s)\n\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
		printf 'Base: %s. Token: length + 4-char prefix only, never the value. The store is never mutated: every call below is a GET, enforced here and re-checked at exit against this run log.\n\n' "$LIVE_BASE"
		printf '| row | GET | code | excerpt |\n|---|---|---|---|\n'
	} >>"$report"

	live_row() {
		local name=$1 path=$2
		live_get "GET|$path"
		printf '| %s | %s | %s | %s |\n' "$name" "$path" "$HTTP_CODE" "${HTTP_BODY:0:120}" >>"$report"
	}

	live_json() {
		# live_json <path> <js-expr> — GET, then extract; die propagates (no subshell).
		local path=$1 expr=$2
		live_get "GET|$path"
		json_field <(printf '%s' "$HTTP_BODY") "$expr"
	}

	live_row "health" "/api/health"
	live_row "tags" "/api/tags"
	live_row "memories" "/api/memories?page=1"
	live_row "stats" "/api/stats"
	live_row "user-profile" "/api/user-profile"
	live_row "user-profiles" "/api/user-profiles"
	live_row "projects" "/api/projects"
	live_row "skills-catalog" "/skills/index.json"

	# The search term: harvest a real word from the first live memory.
	local term
	term=$(live_json "/api/memories?page=1" '(o.data&&o.data[0]&&String(o.data[0].content||"").split(/\s+/)[0])||"probe"')
	[ -n "$term" ] || term="probe"
	log "live search term harvested from live memory content: $term"
	live_row "search" "/api/search?q=$term"

	# A real project id: live roster must be populated (the gate's real-data arm).
	if [ -z "$projid" ]; then
		projid=$(live_json "/api/projects" '(o.data&&o.data[0]&&(o.data[0].projectId||o.data[0].id||o.data[0].path))||""')
	fi
	if [ -z "$projid" ]; then
		LIVE_RC=2
		die "live /api/projects roster is empty - the populated-arm gate cannot pass (report, do not fake)"
	fi
	local enc
	enc=$(node -e 'process.stdout.write(encodeURIComponent(process.argv[1]))' "$projid")
	log "live project id resolved (never printed in full)"
	live_row "sessions" "/api/projects/$enc/sessions"
	live_row "milestones" "/api/projects/$enc/milestones"

	# One catalog file from the live catalog.
	local catfile
	catfile=$(live_json "/skills/index.json" '(o.skills&&o.skills[0]&&o.skills[0].files&&o.skills[0].files[0])||""')
	if [ -n "$catfile" ]; then
		live_row "catalog-file" "/skills/$catfile"
	else
		printf '| catalog-file | - | n/a | live catalog has no skill files |\n' >>"$report"
		log "live catalog has no skill files (recorded, not asserted)"
	fi

	# Populated arms (re-read memories for the count).
	local mem_count
	mem_count=$(live_json "/api/memories?page=1" '(o.data&&o.data.items&&o.data.items.length)||0')
	{
		printf '\n## Populated arms\n\n'
		printf '%s\n' "- memories rows: $mem_count (gate: > 0)"
		printf '%s\n' "- search: q=$term (code and hits in the table above)"
		printf '%s\n' "- project roster: populated (id resolved, sessions/milestones answered)"
	} >>"$report"
	if [ "$mem_count" -eq 0 ]; then
		LIVE_RC=2
		die "populated arm memories rows=$mem_count (gate: > 0)"
	fi

	live_selfcheck
}

live_selfcheck() {
	# The run is invalid if any non-GET was issued: self-check the log.
	# The exit code is preserved: a die() inside live_run must not be turned
	# into a green exit by this EXIT trap.
	local rc="$LIVE_RC"
	if grep -Eq '^LIVECALL (POST|PUT|DELETE|PATCH)' "$RUNLOG" 2>/dev/null; then
		trap - EXIT INT TERM
		die "live GET-only self-check FAILED: a non-GET call is present in $RUNLOG"
	fi
	log "live GET-only self-check: PASS ($LIVECALLS GET calls, zero non-GET)"
	trap - EXIT INT TERM
	if [ "$rc" -ne 0 ]; then
		log "LIVE PASS FAILED (rc=$rc)"
		exit "$rc"
	fi
	log "LIVE PASS COMPLETE"
	exit 0
}

# --------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------
main() {
	if [ "${SMOKE_SELFTEST:-0}" = "1" ]; then
		selftest
		return $?
	fi
	local live_base="" tokenfile="" evidence="docs/evidence/issues-24-29-smoke-20261108" projid=""
	local bin="target/debug/opencode-agents-memory" dist="web-ui/dist"
	while [ $# -gt 0 ]; do
		case $1 in
			--live-base-url) live_base=$2; shift 2 ;;
			--live-token-file) tokenfile=$2; shift 2 ;;
			--evidence-dir) evidence=$2; shift 2 ;;
			--live-project) projid=$2; shift 2 ;;
			--bin) bin=$2; shift 2 ;;
			--dist) dist=$2; shift 2 ;;
			*) printf 'unknown argument: %s\n' "$1" >&2; return 64 ;;
		esac
	done
	if [ -n "$live_base" ]; then
		[ -n "$tokenfile" ] || {
			printf -- '--live-base-url requires --live-token-file\n' >&2
			return 64
		}
		live_run "$live_base" "$tokenfile" "$evidence" "$projid"
	else
		full_run "$bin" "$dist" "$evidence"
	fi
}

main "$@"
