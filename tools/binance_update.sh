#!/usr/bin/env bash
# Download or update Binance spot candles as CSV files in data/, for volcandle.
#
#   tools/binance_update.sh                     update every Binance file in data/
#   tools/binance_update.sh ETHUSDT 1h 1d       download or update ETHUSDT_1h.csv and ETHUSDT_1d.csv
#   tools/binance_update.sh BTCUSDT ETHUSDT 4h  several symbols at once
#   tools/binance_update.sh --since 2025-01-01 SOLUSDT 15m
#                                               new files start at that date (default: first trade)
#
# Existing files only get the candles after their last row. Only finished candles are
# written; the one still forming is left for the next run. volcandle reloads an open
# file by itself. Uses Binance's public market-data API (no account or key needed).
# Intervals: 1s 1m 3m 5m 15m 30m 1h 2h 4h 6h 8h 12h 1d 3d 1w
set -euo pipefail

API=https://data-api.binance.vision/api/v3/klines
HEADER='Open time,Open,High,Low,Close,Volume,Close time,Quote asset volume,Number of trades,Taker buy base asset volume,Taker buy quote asset volume,Ignore'
INTERVALS=' 1s 1m 3m 5m 15m 30m 1h 2h 4h 6h 8h 12h 1d 3d 1w '
DATA_DIR=${VOLCANDLE_DATA:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/data}

usage() { sed -n '2,13p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
die() { echo "binance_update: $*" >&2; exit 2; }
utc() { date -u -d "@$(($1 / 1000))" '+%Y-%m-%d %H:%M'; }

# GET a URL; prints the body, or explains the failure on stderr.
fetch() {
    local out code
    out=$(curl -sS --retry 3 --max-time 30 -w '\n%{http_code}' "$1") || return 1
    code=${out##*$'\n'}
    out=${out%$'\n'*}
    if [[ $code != 200 ]]; then
        echo "Binance answered HTTP $code: $out" >&2
        return 1
    fi
    printf '%s' "$out"
}

# update SYMBOL INTERVAL: create or extend data/SYMBOL_INTERVAL.csv.
update() {
    local sym=$1 iv=$2
    local file="$DATA_DIR/${sym}_${iv}.csv"
    local name="${sym}_${iv}.csv"
    local start=$SINCE_MS last=0
    if [[ -f $file ]]; then
        if [[ $(head -n1 "$file" | tr -d '\r') != "$HEADER" ]]; then
            echo "$name: skipped, it was not made by this script" >&2
            return 1
        fi
        last=$(tail -n1 "$file" | tr -d '\r' | cut -d, -f1)
        if [[ $last =~ ^[0-9]+$ ]]; then start=$((last + 1)); else last=0; fi
    fi

    local new now body batch n
    new=$(mktemp)
    now=$(date +%s%3N)
    while :; do
        if ! body=$(fetch "$API?symbol=$sym&interval=$iv&startTime=$start&limit=1000"); then
            rm -f "$new"
            echo "$name: download failed, nothing written" >&2
            return 1
        fi
        [[ $body == '[]' || -z $body ]] && break
        # [[t,"o","h",...],[...]] -> one CSV line per candle
        batch=$(printf '%s' "$body" | sed -e 's/^\[\[//' -e 's/\]\]$//' -e 's/\],\[/\n/g' -e 's/"//g')
        printf '%s\n' "$batch" | awk -F, -v now="$now" '$7 < now' >>"$new" # finished only
        n=$(printf '%s\n' "$batch" | wc -l)
        start=$(($(printf '%s\n' "$batch" | tail -n1 | cut -d, -f1) + 1))
        ((n < 1000)) && break
    done

    local added
    added=$(wc -l <"$new")
    if ((added == 0)); then
        rm -f "$new"
        if ((last > 0)); then
            echo "$name: up to date (last candle $(utc "$last") UTC)"
        else
            echo "$name: no finished candles yet, nothing written"
        fi
        return 0
    fi

    # Write a complete new file, then swap it in, so volcandle never reads half a file.
    local tmp="$file.part" first final total s=s
    ((added == 1)) && s=
    {
        if [[ -f $file ]]; then tr -d '\r' <"$file" | sed -e '$a\'; else echo "$HEADER"; fi
        cat "$new"
    } >"$tmp"
    mv -f "$tmp" "$file"
    first=$(head -n1 "$new" | cut -d, -f1)
    final=$(tail -n1 "$new" | cut -d, -f1)
    rm -f "$new"
    total=$(($(wc -l <"$file") - 1))
    echo "$name: +$added candle$s ($(utc "$first") .. $(utc "$final") UTC), $total in file"
}

SINCE_MS=0
tokens=()
while (($#)); do
    case $1 in
    -h | --help) usage; exit 0 ;;
    --since | --since=*)
        if [[ $1 == --since ]]; then
            (($# >= 2)) || die "--since needs a date like 2025-01-01"
            d=$2; shift
        else
            d=${1#--since=}
        fi
        s=$(date -u -d "$d" +%s 2>/dev/null) || die "can't read the date '$d' (use YYYY-MM-DD)"
        SINCE_MS=$((s * 1000))
        ;;
    -*) die "unknown option $1 (see --help)" ;;
    *) tokens+=("$1") ;;
    esac
    shift
done

todo=()
if ((${#tokens[@]} == 0)); then
    # No arguments: every file in data/ that this script made.
    for f in "$DATA_DIR"/*_*.csv; do
        [[ -f $f && $(head -n1 "$f" | tr -d '\r') == "$HEADER" ]] || continue
        base=$(basename "$f" .csv)
        todo+=("${base%_*} ${base##*_}")
    done
    if ((${#todo[@]} == 0)); then
        echo "No Binance files in $DATA_DIR yet. Download one with, for example:"
        echo "  $0 BTCUSDT 1h 1d"
        exit 0
    fi
else
    symbols=() intervals=()
    for t in "${tokens[@]}"; do
        if [[ $t == 1M ]]; then
            die "1M (monthly) is not offered: on Windows its file name would clash with 1m. Download 1d and pick 1M in volcandle."
        elif [[ $INTERVALS == *" ${t,,} "* ]]; then
            intervals+=("${t,,}")
        elif [[ $t =~ ^[0-9]+[A-Za-z]+$ ]]; then
            die "unknown interval '$t'. Binance has:${INTERVALS% }"
        else
            t=${t^^}
            symbols+=("${t//[\/_-]/}") # BTC/USDT, btc-usdt -> BTCUSDT
        fi
    done
    ((${#symbols[@]})) || die "name a symbol, e.g. BTCUSDT"
    ((${#intervals[@]})) || intervals=(1h 1d)
    for s in "${symbols[@]}"; do
        for i in "${intervals[@]}"; do todo+=("$s $i"); done
    done
fi

mkdir -p "$DATA_DIR"
failed=0
for j in "${todo[@]}"; do
    # shellcheck disable=SC2086
    update $j || failed=1
done
exit $failed
