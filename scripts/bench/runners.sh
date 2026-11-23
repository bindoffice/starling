#!/usr/bin/env bash

# Sourced by scripts/bench-compare.sh after the server and sampler are running.

run_client() {
    if [[ -n "$CLIENT_LIBRARY_PATH" ]]; then
        env "LD_LIBRARY_PATH=$CLIENT_LIBRARY_PATH${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$@"
    else
        "$@"
    fi
}

peak_resources() {
    local file="$1"
    awk 'NR > 1 {
        if ($1 > rss) rss = $1
        if ($2 > threads) threads = $2
        if ($3 > fds) fds = $3
    } END { print rss + 0 "\t" threads + 0 "\t" fds + 0 }' "$file"
}

normalize_ms() {
    local result
    result="$(awk -v value="$1" '
        function to_ms(text, number, unit) {
            if (!match(text, /^[0-9.]+/)) return text
            number = substr(text, 1, RLENGTH) + 0
            unit = substr(text, RLENGTH + 1)
            if (unit == "us") return number / 1000
            if (unit == "s") return number * 1000
            return number
        }
        BEGIN { print to_ms(value) }
    ')"
    printf '%s' "$result"
}

normalize_mb() {
    local result
    result="$(awk -v value="$1" '
        function to_mb(text, number, unit) {
            if (!match(text, /^[0-9.]+/)) return text
            number = substr(text, 1, RLENGTH) + 0
            unit = substr(text, RLENGTH + 1)
            if (unit == "GB/s" || unit == "GB") return number * 1024
            if (unit == "KB/s" || unit == "KB") return number / 1024
            return number
        }
        BEGIN { print to_mb(value) }
    ')"
    printf '%s' "$result"
}

write_result_row() {
    local implementation="$1"
    local transport="$2"
    local scenario="$3"
    local round="$4"
    local req_s="$5"
    local p50="$6"
    local p99="$7"
    local mean="$8"
    local transfer="$9"
    local requests="${10}"
    local succeeded="${11}"
    local failed="${12}"
    local errored="${13}"
    local timeout="${14}"
    local peak_rss="${15}"
    local peak_threads="${16}"
    local peak_fds="${17}"
    local raw_file="${18}"
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$implementation" "$transport" "$scenario" "$round" "$req_s" "$p50" "$p99" \
        "$mean" "$transfer" "$requests" "$succeeded" "$failed" "$errored" "$timeout" \
        "$peak_rss" "$peak_threads" "$peak_fds" "$raw_file" >>"$SUMMARY_TSV"
}

run_wrk_scenario() {
    local implementation="$1"
    local scenario="$2"
    local path="$3"
    local connections="$4"
    local round raw resources
    raw="$OUTPUT_DIR/$implementation-$scenario-warmup.txt"
    run_client "$WRK_BIN" -t"$THREADS" -c"$connections" -d"$WARMUP" \
        "http://127.0.0.1:$PORT$path" >"$raw"
    for ((round = 1; round <= ROUNDS; round++)); do
        raw="$OUTPUT_DIR/$implementation-$scenario-$round.txt"
        resources="$OUTPUT_DIR/$implementation-$scenario-$round.resources.tsv"
        start_sampler "$SERVER_PID" "$resources"
        run_client "$WRK_BIN" -t"$THREADS" -c"$connections" -d"$DURATION" --latency \
            "http://127.0.0.1:$PORT$path" >"$raw"
        stop_sampler
        local parsed
        parsed="$(awk '
            /Requests\/sec:/ { req_s = $2 }
            /Transfer\/sec:/ { transfer = $2 }
            /^[[:space:]]+50%/ { p50 = $2 }
            /^[[:space:]]+99%/ { p99 = $2 }
            /requests in/ { requests = $1 }
            END {
                print req_s "\t" p50 "\t" p99 "\t" transfer "\t" requests
            }
        ' "$raw")"
        IFS=$'\t' read -r req_s p50_raw p99_raw transfer_raw requests <<<"$parsed"
        local p50 p99 transfer peak
        p50="$(normalize_ms "$p50_raw")"
        p99="$(normalize_ms "$p99_raw")"
        transfer="$(normalize_mb "$transfer_raw")"
        peak="$(peak_resources "$resources")"
        IFS=$'\t' read -r peak_rss peak_threads peak_fds <<<"$peak"
        write_result_row "$implementation" http1 "$scenario" "$round" "$req_s" \
            "$p50" "$p99" "-" "$transfer" "$requests" "$requests" "0" "0" "0" \
            "$peak_rss" "$peak_threads" "$peak_fds" "$raw"
    done
}

run_h2_scenario() {
    local implementation="$1"
    local round raw resources
    raw="$OUTPUT_DIR/$implementation-h2-small-warmup.txt"
    run_client "$H2LOAD_BIN" -t"$THREADS" -c"$H2_CLIENTS" -m"$H2_STREAMS" \
        -n"$((H2_REQUESTS / 10 > 0 ? H2_REQUESTS / 10 : 1))" \
        "http://127.0.0.1:$PORT/small" >"$raw"
    for ((round = 1; round <= ROUNDS; round++)); do
        raw="$OUTPUT_DIR/$implementation-h2-small-$round.txt"
        resources="$OUTPUT_DIR/$implementation-h2-small-$round.resources.tsv"
        start_sampler "$SERVER_PID" "$resources"
        run_client "$H2LOAD_BIN" -t"$THREADS" -c"$H2_CLIENTS" -m"$H2_STREAMS" -n"$H2_REQUESTS" \
            "http://127.0.0.1:$PORT/small" >"$raw"
        stop_sampler
        local parsed
        parsed="$(awk '
            /finished in/ {
                for (i = 1; i <= NF; i++) {
                    if ($i == "req/s,") req_s = $(i - 1)
                    if ($i ~ /MB\/s/) {
                        transfer = $i
                        sub(/,$/, "", transfer)
                    }
                }
            }
            /^requests:/ {
                for (i = 1; i <= NF; i++) {
                    if ($i == "total,") requests = $(i - 1)
                    if ($i == "succeeded,") succeeded = $(i - 1)
                    if ($i == "failed,") failed = $(i - 1)
                    if ($i == "errored,") errored = $(i - 1)
                    if ($i == "timeout") timeout = $(i - 1)
                }
            }
            /time for request:/ { mean = $6 }
            END {
                gsub(/,/, "", requests)
                gsub(/,/, "", succeeded)
                gsub(/,/, "", failed)
                gsub(/,/, "", errored)
                gsub(/,/, "", timeout)
                print req_s "\t" mean "\t" transfer "\t" requests "\t" succeeded \
                    "\t" failed "\t" errored "\t" timeout
            }
        ' "$raw")"
        IFS=$'\t' read -r req_s mean_raw transfer_raw requests succeeded failed errored timeout \
            <<<"$parsed"
        mean="$(normalize_ms "$mean_raw")"
        transfer="$(normalize_mb "$transfer_raw")"
        peak="$(peak_resources "$resources")"
        IFS=$'\t' read -r peak_rss peak_threads peak_fds <<<"$peak"
        write_result_row "$implementation" h2c small "$round" "$req_s" "-" "-" "$mean" \
            "$transfer" "$requests" "$succeeded" "$failed" "$errored" "$timeout" \
            "$peak_rss" "$peak_threads" "$peak_fds" "$raw"
    done
}
