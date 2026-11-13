#!/usr/bin/env bash
set -euo pipefail
for f in *.c; do
	gcc -O2 -o "${f%.c}" "$f" && echo "編譯完成：$f"
done
