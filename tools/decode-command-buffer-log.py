#!/usr/bin/env python3
"""Reassemble NativeAdrenoMetal command-buffer messages from compact system logs."""
import argparse
import base64
import json
from pathlib import Path
import re
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('logs', type=Path, nargs='+', help='log files from one boot')
    parser.add_argument('--functions', type=Path, help='write captured function AIR and its names to this directory')
    args = parser.parse_args()
    pattern = re.compile(r'(\S+)\[(\d+):[^]]+\].*NativeAdrenoMetal CB dump '
                         r'(\d+) (\d+)/(\d+): (.*)')
    entries = {}
    raw_sequence = 0
    for path in args.logs:
        for line in path.read_text(errors='replace').splitlines():
            if line.startswith('native-metal dump '):
                raw_sequence += 1
                entries['stderr', 0, raw_sequence] = (1, {1: line})
                continue
            if 'NativeAdrenoMetal CB dump ' not in line:
                continue
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
    functions = []
    if args.functions:
        args.functions.mkdir(parents=True, exist_ok=True)
    for (process, pid, sequence), (count, chunks) in sorted(entries.items()):
        missing = set(range(1, count + 1)) - chunks.keys()
        if missing:
            incomplete += 1
            print(f'{process}[{pid}] entry {sequence}: missing fragments {sorted(missing)}', file=sys.stderr)
            continue
        text = ''.join(chunks[part] for part in range(1, count + 1))
        if text.strip() == 'native-metal dump capture limit reached':
            incomplete += 1
            print('command-buffer capture reached its file limit', file=sys.stderr)
        function = re.fullmatch(r'native-metal dump function (\d+) (\S+): (.*)', text)
        if function and args.functions:
            index, name, encoded = function.groups()
            try:
                data = base64.b64decode(encoded, validate=True)
            except ValueError:
                incomplete += 1
                print(f'{process}[{pid}] function {name}: {encoded[:120]}', file=sys.stderr)
                continue
            filename = f'function-{pid}-{index}.air'
            (args.functions / filename).write_bytes(data)
            functions.append(dict(process=process, pid=pid, name=name, file=filename))
            print(f'{process}[{pid}] function {name}: {filename}, {len(data)} bytes')
        else:
            print(f'{process}[{pid}] ' + text)
    if args.functions:
        (args.functions / 'functions.json').write_text(json.dumps(functions, indent=2) + '\n')
    print(f'command-buffer entries={len(entries)} incomplete={incomplete}', file=sys.stderr)
    return bool(incomplete)


if __name__ == '__main__':
    sys.exit(main())
