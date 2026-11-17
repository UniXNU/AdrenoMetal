#!/usr/bin/env python3
"""Report runtime policies, driver conditions, float controls and Turnip options from their sources.

Use --runtime-policy with a captured runtime-policy.log to attach the device's actual
profile and check verdicts. Source references describe gates; they do not establish
that the device executed a path. Math operations use upstream Math::float_stems(),
BinOp::ALL and PrecisionNote's existing classification and validation.
"""
import argparse
import ast
import base64
import hashlib
import json
from pathlib import Path
import re
import subprocess
import urllib.request

COMPONENT = Path(__file__).resolve().parents[1]
CORE = COMPONENT.parents[1]
MESA_FILES = ('src/freedreno/vulkan/tu_drirc_gen.py',
              'src/freedreno/vulkan/tu_device.cc',
              'src/freedreno/vulkan/tu_cmd_buffer.cc',
              'src/freedreno/vulkan/tu_sampler.cc',
              'src/freedreno/vulkan/tu_lrz.cc',
              'src/freedreno/ir3/ir3_compiler.c',
              'src/freedreno/ir3/ir3_compiler_nir.c')


def mask(text):
    return re.sub(r'//[^\n]*|/\*[\s\S]*?\*/|"(?:\\.|[^"\\])*"',
                  lambda m: ''.join('\n' if c == '\n' else ' ' for c in m[0]), text)


def body(text, pattern):
    match = re.search(pattern, mask(text))
    if not match:
        raise ValueError('Source declaration missing: ' + pattern)
    clean = mask(text)
    at = clean.index('{', match.end())
    depth = 1
    end = at + 1
    while depth:
        if clean[end] == '{':
            depth += 1
        elif clean[end] == '}':
            depth -= 1
        end += 1
    return text[at + 1:end - 1], at + 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--core', type=Path, default=CORE, help='UniXNU checkout owning the pinned Mesa inputs')
    parser.add_argument('--translator', type=Path, default=CORE / '.cache/gpu-reference/metal2vulkan')
    parser.add_argument('--mesa-source', type=Path, default=CORE / '.cache/mesa-work')
    parser.add_argument('--fetch-mesa', action='store_true', help='Cache only the pinned Mesa source texts this report reads')
    parser.add_argument('--runtime-policy', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    core = args.core.resolve()
    mesa_pin = json.loads((core / 'scripts/data/adreno-compiler-inputs.json').read_text())['mesa']
    mesa = args.mesa_source
    if args.fetch_mesa:
        mesa = core / '.cache/runtime-fast-path-audit' / mesa_pin['commit']
        for name in MESA_FILES:
            path = mesa / name
            if path.is_file():
                continue
            url = mesa_pin['mirror'].rstrip('/') + '/+/' + mesa_pin['commit'] + '/' + name + '?format=TEXT'
            with urllib.request.urlopen(url, timeout=45) as response:
                data = base64.b64decode(response.read(), validate=True)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
    files = {}

    def read(root, name):
        path = root / name
        text = path.read_text()
        files[str(path)] = hashlib.sha256(text.encode()).hexdigest()
        return text

    policy_path = 'crates/metal2vulkan/src/device/policy.rs'
    policy = read(args.translator, policy_path)
    struct, _ = body(policy, r'pub struct DevicePolicy\b')
    fields = re.findall(r'^    pub (\w+): ([^,]+),', struct, re.M)
    defaults, _ = body(policy, r'pub const UNTRUSTED: DevicePolicy = DevicePolicy')
    defaults = dict(re.findall(r'^        (\w+): (.+),$', defaults, re.M))
    consumer = read(COMPONENT, 'bridge/NativeMetalObjects.rs')
    overrides = dict(re.findall(r'options\.policy\.(\w+) = ([^;]+);', consumer))
    references = {name: [] for name, _ in fields}
    match_policy = re.compile(r'\.\s*(' + '|'.join(references) + r')\b')
    for folder in ('crates/metal2vulkan/src', 'crates/metal2vulkan-runtime/src'):
        for path in (args.translator / folder).rglob('*.rs'):
            if '/tests/' in str(path):
                continue
            text = path.read_text()
            for line, value in enumerate(text.splitlines(), 1):
                for match in match_policy.finditer(value):
                    references[match[1]].append(dict(file=str(path.relative_to(args.translator)), line=line, expression=value.strip()))
    profile_text = read(args.translator, 'crates/metal2vulkan/src/device/profile.rs')
    ledger = read(args.translator, 'docs/FAST-PATHS.md').splitlines()
    rows = []
    for name, kind in fields:
        pattern = r'pub enum ' + re.escape(kind) + r'\b'
        owner = next((source for source in (policy, profile_text) if re.search(pattern, mask(source))), None)
        declaration = body(owner, pattern)[0] if owner else ''
        rows.append(dict(field=name, type=kind, upstream_default=defaults.get(name),
                         consumer_override=overrides.get(name), source=policy_path,
                         values=re.findall(r'^    ([A-Z]\w*)\s*(?:,|\{|\()', mask(declaration), re.M),
                         references=references[name],
                         accuracy_ledger=[dict(file='docs/FAST-PATHS.md', line=n, expression=line)
                                          for n, line in enumerate(ledger, 1)
                                          if re.search(r'\b' + name + r'\s*=', line)]))
    vocabulary = {}
    for name, path in (
        ('FloatWidth', 'crates/metal2vulkan/src/device/requirements.rs'),
        ('FloatControl', 'crates/metal2vulkan/src/device/requirements.rs'),
        ('FloatFunctions', 'crates/metal2vulkan/src/device/profile.rs'),
        ('FloatWidthControls', 'crates/metal2vulkan/src/device/profile.rs'),
        ('FloatControls', 'crates/metal2vulkan/src/device/profile.rs'),
        ('ProfileFields', 'crates/metal2vulkan/src/device/profile.rs'),
        ('LeafCaps', 'crates/metal2vulkan/src/device/profile.rs'),
        ('PrecisionNote', 'crates/metal2vulkan/src/pipeline/artifact.rs'),
    ):
        text = read(args.translator, path)
        contents, _ = body(text, r'pub (?:enum|struct) ' + name + r'\b')
        members = re.findall(r'^    pub (\w+):', contents, re.M)
        if not members:
            members = re.findall(r'^    ([A-Z]\w*)\s*(?:,|\{|\()', mask(contents), re.M)
        vocabulary[name] = dict(source=path, count=len(members), members=members)
    driver_conditions = {}
    for name, path in (
        ('DriverDeviation', 'crates/metal2vulkan/src/error/mod.rs'),
        ('DriverWorkaround', 'crates/metal2vulkan/src/device/profile.rs'),
        ('DriverFact', 'crates/metal2vulkan/src/device/driver_facts.rs'),
    ):
        text = read(args.translator, path)
        contents, offset = body(text, r'pub enum ' + name + r'\b')
        variants = {}
        for match in re.finditer(r'^    ([A-Z]\w*)\s*(?:,|\{|\()', mask(contents), re.M):
            variants[match[1]] = dict(line=text.count('\n', 0, offset + match.start()) + 1,
                                      qualified_references=[])
        driver_conditions[name] = dict(source=path, count=len(variants), variants=variants)
    qualified_condition = re.compile(r'\b(' + '|'.join(driver_conditions) + r')\s*::\s*(\w+)\b')
    for folder in ('crates/metal2vulkan/src', 'crates/metal2vulkan-runtime/src'):
        for path in (args.translator / folder).rglob('*.rs'):
            if '/tests/' in str(path):
                continue
            lines = path.read_text().splitlines()
            for line, value in enumerate(mask('\n'.join(lines)).splitlines(), 1):
                for match in qualified_condition.finditer(value):
                    variant = driver_conditions[match[1]]['variants'].get(match[2])
                    if variant is not None:
                        variant['qualified_references'].append(dict(
                            file=str(path.relative_to(args.translator)), line=line,
                            expression=lines[line - 1].strip()))
    gates = []
    for path, names in (
        ('crates/metal2vulkan/src/lower/arith/narrow.rs', ('relaxes', 'quantize_form', 'half_controls', 'controlled_modes')),
        ('crates/metal2vulkan/src/lower/arith/fma.rs', ('float_route', 'half_route')),
        ('crates/metal2vulkan/src/lower/arith/mod.rs', ('contraction', 'fp_env')),
        ('crates/metal2vulkan-runtime/src/batch_order.rs', ('ends_order',)),
    ):
        text = read(args.translator, path)
        for name in names:
            pattern = r'\bfn ' + name + r'\b'
            if name == 'half_controls' and not re.search(pattern, mask(text)):
                gates.append(dict(file=path, symbol=name, available=False))
                continue
            contents, offset = body(text, pattern)
            gates.append(dict(file=path, symbol=name, available=True,
                              line=text.count('\n', 0, offset) + 1, expression=contents.strip()))
    mesa_text = {}
    for name in MESA_FILES:
        mesa_text[name] = read(mesa, name)
    registry_path = 'src/freedreno/vulkan/tu_drirc_gen.py'
    registry = mesa_text[registry_path]
    options = []
    for node in ast.walk(ast.parse(registry)):
        if not isinstance(node, ast.Assign) or not isinstance(node.value, ast.List):
            continue
        group = next((target.id for target in node.targets if isinstance(target, ast.Name) and target.id.endswith('_options')), None)
        if not group:
            continue
        for call in node.value.elts:
            if not isinstance(call, ast.Call) or not call.args or not isinstance(call.args[0], ast.Constant):
                continue
            options.append(dict(group=group, name=call.args[0].value,
                                default=ast.get_source_segment(registry, call.args[1]) if len(call.args) > 1 else None,
                                source=registry_path, line=call.lineno,
                                declaration=ast.get_source_segment(registry, call)))
    property_lines = []
    selectors = re.compile(r'shader(?:Float16|FloatControls2|Denorm\w+|Rounding\w+|SignedZero\w+)|driverID|vendorID|deviceID|lower_fdiv|float_mul_add\d+')
    for name, text in mesa_text.items():
        for line, value in enumerate(text.splitlines(), 1):
            if selectors.search(value):
                property_lines.append(dict(file=name, line=line, expression=value.strip()))
    plugin = read(COMPONENT, 'NativeAdrenoMetalPlugin.mm')
    result = dict(schema='unixnu-runtime-fast-path-audit-1',
                  translator=subprocess.check_output(['git', '-C', str(args.translator), 'rev-parse', 'HEAD'], text=True).strip(),
                  mesa=mesa_pin['commit'], policy_field_count=len(rows), policies=rows,
                  float_vocabulary=vocabulary, driver_conditions=driver_conditions,
                  driver_condition_count=sum(row['count'] for row in driver_conditions.values()),
                  owning_gates=gates,
                  mesa_option_count=len(options), mesa_options=options, mesa_property_evidence=property_lines,
                  consumer_turnip_configuration=[line.strip() for line in plugin.splitlines() if 'setenv("tu_' in line],
                  math_enumeration='Upstream Math::float_stems(), BinOp::ALL, PrecisionNote::of_math_call/of_float_operator, engine_float_call_notes and fast_paths; no separate operation list.',
                  evidence='Finite source declarations and gate expressions. Runtime verdicts and active paths come from runtime-policy.log and timing/FP counters.',
                  source_files=files,
                  unresolved_policy_defaults=[row['field'] for row in rows if row['upstream_default'] is None])
    if args.runtime_policy:
        text = args.runtime_policy.read_text()
        result['runtime_capture'] = dict(file=str(args.runtime_policy), stamp=text.splitlines()[0], contents=text)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print('policy-fields=%d Turnip-options=%d driver-conditions=%d unresolved-policy-defaults=%d -> %s' % (
        len(rows), len(options), result['driver_condition_count'], len(result['unresolved_policy_defaults']), args.output))


if __name__ == '__main__':
    main()
