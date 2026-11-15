#!/usr/bin/env python3
"""Reassemble NativeAdrenoMetal command-buffer messages from compact system logs."""
import argparse
from pathlib import Path
import re
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('logs', type=Path, nargs='+', help='log files from one boot')
    args = parser.parse_args()
    pattern = re.compile(r'(\S+)\[(\d+):[^]]+\].*NativeAdrenoMetal CB dump '
                         r'(\d+) (\d+)/(\d+): (.*)')
    entries = {}
    for path in args.logs:
        for line in path.read_text(errors='replace').splitlines():
            match = pattern.search(line)
            if not match:
                continue
            process, pid, sequence, part, count, text = match.groups()
            key = process, int(pid), int(sequence)
            part, count = int(part), int(count)
            expected, chunks = entries.setdefault(key, (count, {}))
            if count != expected or not 1 <= part <= count or (part in chunks and chunks[part] != text):
                parser.error(f'conflicting log fragments for {process}[{pid}] entry {sequence}')
            chunks[part] = text
    if not entries:
        parser.error('no command-buffer messages found')
    incomplete = 0
    for (process, pid, sequence), (count, chunks) in sorted(entries.items()):
        missing = set(range(1, count + 1)) - chunks.keys()
        if missing:
            incomplete += 1
            print(f'{process}[{pid}] entry {sequence}: missing fragments {sorted(missing)}', file=sys.stderr)
            continue
        print(f'{process}[{pid}] ' + ''.join(chunks[part] for part in range(1, count + 1)))
    print(f'command-buffer entries={len(entries)} incomplete={incomplete}', file=sys.stderr)
    return bool(incomplete)


if __name__ == '__main__':
    sys.exit(main())
