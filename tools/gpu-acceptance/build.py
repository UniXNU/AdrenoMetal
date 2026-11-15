#!/usr/bin/env python3
"""Build the bounded A740 GPU acceptance executable with embedded AIR and native SPIR-V controls."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

COMPONENT = Path(__file__).resolve().parents[2]
CORE = COMPONENT.parents[1]
HERE = Path(__file__).resolve().parent


def run(args, **kwargs):
    return subprocess.run([str(x) for x in args], check=True, **kwargs)


def mask(source):
    result = list(source)
    i = 0
    while i < len(source):
        end = i
        if source.startswith('//', i):
            end = source.find('\n', i)
            if end < 0: end = len(source)
        elif source.startswith('/*', i):
            depth, end = 1, i + 2
            while depth:
                if source.startswith('/*', end): depth += 1; end += 2
                elif source.startswith('*/', end): depth -= 1; end += 2
                else: end += 1
        elif source[i] == '"':
            end = i + 1
            while end < len(source):
                if source[end] == '\\': end += 2
                elif source[end] == '"': end += 1; break
                else: end += 1
        if end > i:
            for at in range(i, end):
                if result[at] != '\n': result[at] = ' '
            i = end
        else: i += 1
    return ''.join(result)


def item(source, name, kind='fn'):
    clean = mask(source)
    start = re.search(r'^' + kind + r' ' + name + r'\b', clean, re.M).start()
    first = clean.index('{', start)
    depth, end = 1, first + 1
    while depth:
        if clean[end] == '{': depth += 1
        elif clean[end] == '}': depth -= 1
        end += 1
    return source[start:end] + '\n'


def arithmetic_air(half):
    f = 'half' if half else 'float'
    v = '<4 x ' + f + '>'
    a = '0xH3BFE' if half else '9.990234375e-01'
    b = '0xH1400' if half else '9.765625e-04'
    vec = lambda value: '<' + ', '.join(f + ' ' + value for _ in range(4)) + '>'
    narrow = '%initial = fptrunc <4 x float> %loaded to <4 x half>' if half else '%initial = fadd <4 x float> %loaded, zeroinitializer'
    widen = '%answer = fpext <4 x half> %next to <4 x float>' if half else '%answer = fadd <4 x float> %next, zeroinitializer'
    return '''target datalayout = "e-p:64:64:64-i1:8:8-i8:8:8-i16:16:16-i32:32:32-i64:64:64-f32:32:32-f64:64:64-v16:16:16-v24:32:32-v32:32:32-v48:64:64-v64:64:64-v96:128:128-v128:128:128-v192:256:256-v256:256:256-v512:512:512-v1024:1024:1024-n8:16:32"
target triple = "air64_v28-apple-macosx26.0.0"
define void @arith(ptr addrspace(1) noundef "air-buffer-no-alias" %in, ptr addrspace(1) noundef "air-buffer-no-alias" %out, i32 noundef %gid) {
entry:
  %g = zext i32 %gid to i64
  %src = getelementptr inbounds <4 x float>, ptr addrspace(1) %in, i64 %g
  %loaded = load <4 x float>, ptr addrspace(1) %src, align 16
  ''' + narrow + '''
  br label %loop
loop:
  %i = phi i32 [0, %entry], [%j, %loop]
  %acc = phi ''' + v + ''' [%initial, %entry], [%next, %loop]
  %product = fmul fast ''' + v + ' %acc, ' + vec(a) + '''
  %next = fadd fast ''' + v + ' %product, ' + vec(b) + '''
  %j = add i32 %i, 1
  %again = icmp ult i32 %j, 256
  br i1 %again, label %loop, label %exit
exit:
  ''' + widen + '''
  %dst = getelementptr inbounds <4 x float>, ptr addrspace(1) %out, i64 %g
  store <4 x float> %answer, ptr addrspace(1) %dst, align 16
  ret void
}
!air.kernel = !{!0}
!air.version = !{!6}
!air.language_version = !{!7}
!0 = !{ptr @arith, !1, !2}
!1 = !{}
!2 = !{!3, !4, !5}
!3 = !{i32 0, !"air.buffer", !"air.location_index", i32 0, i32 1, !"air.read", !"air.address_space", i32 1, !"air.arg_type_size", i32 16, !"air.arg_type_align_size", i32 16, !"air.arg_type_name", !"float4", !"air.arg_name", !"in"}
!4 = !{i32 1, !"air.buffer", !"air.location_index", i32 1, i32 1, !"air.read_write", !"air.address_space", i32 1, !"air.arg_type_size", i32 16, !"air.arg_type_align_size", i32 16, !"air.arg_type_name", !"float4", !"air.arg_name", !"out"}
!5 = !{i32 2, !"air.thread_position_in_grid", !"air.arg_type_name", !"uint", !"air.arg_name", !"gid"}
!6 = !{i32 2, i32 8, i32 0}
!7 = !{!"Metal", i32 4, i32 0, i32 0}
'''


def control_spirv(half=False, scale=False):
    # The arithmetic control has the AIR's four independent lanes and bounded loop. The
    # bandwidth control is runtime_examples.ll's scale_and_offset: out[i] = in[i] * scale + i.
    text = '''OpCapability Shader
'''
    if half: text += 'OpCapability Float16\n'
    text += '''OpMemoryModel Logical GLSL450
OpEntryPoint GLCompute %main "main" %gid %input %output''' + (' %scale' if scale else '') + '''
OpExecutionMode %main LocalSize 64 1 1
OpDecorate %gid BuiltIn GlobalInvocationId
OpDecorate %arr ArrayStride ''' + ('4' if scale else '16') + '''
OpMemberDecorate %Block 0 Offset 0
OpDecorate %Block Block
OpDecorate %input DescriptorSet 0
OpDecorate %input Binding 0
OpDecorate %output DescriptorSet 0
OpDecorate %output Binding 1
'''
    if scale: text += '''OpDecorate %scale DescriptorSet 0
OpDecorate %scale Binding 2
'''
    text += '''%void = OpTypeVoid
%fn = OpTypeFunction %void
%uint = OpTypeInt 32 0
%bool = OpTypeBool
%v3uint = OpTypeVector %uint 3
%ptr_gid = OpTypePointer Input %v3uint
%gid = OpVariable %ptr_gid Input
%float = OpTypeFloat 32
%v4float = OpTypeVector %float 4
'''
    if half: text += '%half = OpTypeFloat 16\n%v4half = OpTypeVector %half 4\n'
    value = '%float' if scale else '%v4float'
    arithmetic = '%v4half' if half else '%v4float'
    text += '%arr = OpTypeRuntimeArray ' + value + '''
%Block = OpTypeStruct %arr
%ptr_block = OpTypePointer StorageBuffer %Block
%ptr_value = OpTypePointer StorageBuffer ''' + value + '''
%input = OpVariable %ptr_block StorageBuffer
%output = OpVariable %ptr_block StorageBuffer
'''
    if scale: text += '%scale = OpVariable %ptr_block StorageBuffer\n'
    text += '''%zero = OpConstant %uint 0
%one = OpConstant %uint 1
%limit = OpConstant %uint 256
'''
    if not scale:
        scalar = '%half' if half else '%float'
        text += '%a = OpConstant ' + scalar + ' 0.9990234375\n%b = OpConstant ' + scalar + ' 0.0009765625\n'
        text += '%va = OpConstantComposite ' + arithmetic + ' %a %a %a %a\n%vb = OpConstantComposite ' + arithmetic + ' %b %b %b %b\n'
    text += '''%main = OpFunction %void None %fn
%entry = OpLabel
%ids = OpLoad %v3uint %gid
%index = OpCompositeExtract %uint %ids 0
%src = OpAccessChain %ptr_value %input %zero %index
%dst = OpAccessChain %ptr_value %output %zero %index
%loaded = OpLoad ''' + value + ''' %src
'''
    if scale:
        text += '''%sf = OpAccessChain %ptr_value %scale %zero %zero
%factor = OpLoad %float %sf
%product = OpFMul %float %loaded %factor
%fi = OpConvertUToF %float %index
%answer = OpFAdd %float %product %fi
OpStore %dst %answer
OpReturn
OpFunctionEnd
'''
    else:
        text += ('%initial = OpFConvert %v4half %loaded\n' if half else '%initial = OpCopyObject %v4float %loaded\n')
        text += '''OpBranch %loop
%loop = OpLabel
%i = OpPhi %uint %zero %entry %j %body
%acc = OpPhi ''' + arithmetic + ''' %initial %entry %next %body
OpLoopMerge %exit %body None
OpBranch %body
%body = OpLabel
%product = OpFMul ''' + arithmetic + ''' %acc %va
%next = OpFAdd ''' + arithmetic + ''' %product %vb
%j = OpIAdd %uint %i %one
%again = OpULessThan %bool %j %limit
OpBranchConditional %again %loop %exit
%exit = OpLabel
'''
        text += ('%answer = OpFConvert %v4float %next\n' if half else '%answer = OpCopyObject %v4float %next\n')
        text += 'OpStore %dst %answer\nOpReturn\nOpFunctionEnd\n'
    return text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--translator', type=Path, default=CORE / '.cache/gpu-reference/metal2vulkan')
    parser.add_argument('--target', choices=['host', 'macos27'], default='macos27')
    parser.add_argument('--vulkan-include', type=Path, default=CORE / '.cache/mesa-work/include')
    parser.add_argument('--output', type=Path, default=CORE / 'build/gpu-acceptance')
    args = parser.parse_args()
    out = args.output.resolve(); out.mkdir(parents=True, exist_ok=True)
    source = args.translator.resolve()
    pin = subprocess.check_output(['git', '-C', source, 'rev-parse', 'HEAD'], text=True).strip()
    expected = json.loads((CORE / 'scripts/data/gpu-metal-vulkan-inputs.json').read_text())['translator']['commit']
    if pin != expected: raise SystemExit('Acceptance translator differs from the product source pin')
    upstream = source / 'crates/metal2vulkan-validation/src/runtime_lane/native_maths.rs'
    maths = upstream.read_text()
    helpers = ['operands','ulp','half_ulp','excess','broken','to_half','metal_half','half_operands','half_operand','half_within','half_within_sum']
    constants = ''.join(re.search(r'^const ' + name + r':[^\n]+', maths, re.M)[0]+'\n' for name in ['IN','OUT','COUNT','PAYLOAD_NAN'])
    stream_start = maths.index('struct Stream(')
    stream = maths[stream_start:maths.index('const PAYLOAD_NAN', stream_start)]
    numerical = constants + stream + ''.join(item(maths, name).replace('fn '+name, 'pub fn '+name, 1) if name in ['operands','broken','metal_half','half_operands','half_within','half_within_sum','half_ulp'] else item(maths,name) for name in helpers)
    (out/'numerical.rs').write_text(numerical)
    kernels = ['kernel','half_kernel','half_arithmetic_kernel']
    generator = constants + ''.join(item(maths, name) for name in kernels)
    generator += '\nfn main() { let root=std::path::PathBuf::from(std::env::args().nth(1).unwrap());'
    for f,name in zip(kernels,['native_maths','native_half_rounding','native_half_arithmetic']):
        generator += 'std::fs::write(root.join("'+name+'.ll"), '+f+'()).unwrap();'
    generator += '}\n'
    (out/'kernels.rs').write_text(generator)
    run(['rustc','+1.98.0','--edition=2021','-A','dead_code',out/'kernels.rs','-o',out/'kernels'])
    run([out/'kernels',out])
    (out/'native_half_division.ll').write_text((out/'native_half_rounding.ll').read_text().replace('%hs = fadd half', '%hs = fdiv half'))
    (out/'arith32.ll').write_text(arithmetic_air(False));(out/'arith16.ll').write_text(arithmetic_air(True))
    llvm = shutil.which('llvm-as') or '/opt/homebrew/opt/llvm/bin/llvm-as'
    for name in ['native_maths','native_half_rounding','native_half_arithmetic','native_half_division','arith32','arith16']:
        run([llvm,out/(name+'.ll'),'-o',out/(name+'.air')])
    run([llvm,source/'crates/metal2vulkan-validation/fixtures/public/examples/runtime_examples.ll','-o',out/'runtime_examples.air'])
    for name,half,scale in [('arith32',False,False),('arith16',True,False),('scale',False,True)]:
        (out/(name+'.spvasm')).write_text(control_spirv(half,scale))
        run(['spirv-as','--target-env','vulkan1.2',out/(name+'.spvasm'),'-o',out/(name+'.spv')])
        run(['spirv-val','--target-env','vulkan1.2',out/(name+'.spv')])
    component = subprocess.check_output(['git','-C',COMPONENT,'rev-parse','HEAD'],text=True).strip()
    manifest = '\n'.join(['[package]','name="gpu-acceptance"','version="0.1.0"','edition="2024"','[workspace]',
        '[lib]','crate-type=["staticlib"]','path='+json.dumps(str(HERE/'lib.rs')),'[dependencies]',
        'metal2vulkan={path='+json.dumps(str(source/'crates/metal2vulkan'))+',features=["ash"]}',
        'metal2vulkan-runtime={path='+json.dumps(str(source/'crates/metal2vulkan-runtime'))+'}',
        'ash={version="0.38",default-features=false,features=["loaded"]}','serde_json="1"','libc="0.2"',
        '[profile.release]','panic="unwind"','debug=1','lto="fat"','codegen-units=1',''])
    (out/'Cargo.toml').write_text(manifest)
    triple = 'arm64e-apple-darwin' if args.target=='macos27' else 'aarch64-apple-darwin'
    env=dict(os.environ,NMB_ACCEPTANCE_NUMERIC=str(out/'numerical.rs'),NMB_ACCEPTANCE_INPUTS=str(out),
             NMB_COMPONENT_ID=component,NMB_TRANSLATOR_ID=pin,MACOSX_DEPLOYMENT_TARGET='27.0' if args.target=='macos27' else '14.0')
    cargo=['cargo','+1.98.0']
    if args.target=='macos27': env['RUSTC_BOOTSTRAP']='1';cargo+=['-Z','build-std=std,panic_unwind']
    with (out/'build.log').open('w') as stream:
        run(cargo+['build','--release','--manifest-path',out/'Cargo.toml','--target',triple,'--target-dir',CORE/'build/m2v-target'],env=env,cwd=CORE,stdout=stream,stderr=subprocess.STDOUT)
    archive=CORE/'build/m2v-target'/triple/'release/libgpu_acceptance.a'
    sdk=subprocess.check_output(['xcrun','--sdk','macosx','--show-sdk-path'],text=True).strip()
    cc=['xcrun','clang','-target','arm64e-apple-macos27.0' if args.target=='macos27' else 'arm64-apple-macos14.0','-isysroot',sdk,'-O2','-Wall','-Wextra','-Werror']
    extra=[]
    if args.target=='macos27':
        renames={'_pthread_create':'_nmb_pac_pthread_create','__tlv_atexit':'_nmb_pac_tlv_atexit','_atexit':'_nmb_pac_atexit','___cxa_atexit':'_nmb_pac_cxa_atexit','__dyld_register_func_for_add_image':'_nmb_pac_dyld_register_func_for_add_image','__Unwind_Backtrace':'_nmb_pac_Unwind_Backtrace','_dlsym':'_nmb_pac_dlsym'}
        objcopy=shutil.which('llvm-objcopy') or '/opt/homebrew/opt/llvm/bin/llvm-objcopy'
        pac=out/'libgpu_acceptance_pac.a'
        run([objcopy,*['--redefine-sym='+a+'='+b for a,b in renames.items()],archive,pac]);archive=pac
        run(cc+['-std=c11','-I',args.vulkan_include,'-c',COMPONENT/'bridge/RustPacBoundary.c','-o',out/'RustPacBoundary.o'])
        extra=[out/'RustPacBoundary.o','-Wl,-map,'+str(out/'gpu-acceptance.map')]
    exe=out/'gpu-acceptance'
    run(cc+['-std=c11',HERE/'main.c',archive,*extra,'-framework','Foundation','-framework','Metal','-framework','IOSurface','-lobjc','-liconv','-Wl,-dead_strip','-o',exe])
    if args.target=='macos27':run(['python3',CORE/'scripts/rust-pac-rewrite.py',exe,out/'gpu-acceptance.map',archive.name])
    run(['strip','-x',exe])
    run(['codesign','--force','--sign','-','--pagesize','4096',exe])
    if args.target=='host': run([exe,'--describe'])
    (out/'gpu-acceptance.json').write_text(json.dumps(dict(component=component,translator=pin,numerical_source_sha256=hashlib.sha256(upstream.read_bytes()).hexdigest(),target=triple,sha256=hashlib.sha256(exe.read_bytes()).hexdigest()),indent=2)+'\n')
    package=out/'files'
    (package/'bin').mkdir(parents=True,exist_ok=True)
    (package/'share/gpu-acceptance').mkdir(parents=True,exist_ok=True)
    shutil.copy2(exe,package/'bin/gpu-acceptance')
    shutil.copyfile(out/'gpu-acceptance.json',package/'share/gpu-acceptance/build.json')
    shutil.copyfile(COMPONENT/'LICENSE',package/'share/gpu-acceptance/LICENSE')
    for license in source.glob('LICENSE*'):
        if license.is_file():shutil.copyfile(license,package/'share/gpu-acceptance'/('metal2vulkan-'+license.name))
    print(exe)


if __name__=='__main__': main()
