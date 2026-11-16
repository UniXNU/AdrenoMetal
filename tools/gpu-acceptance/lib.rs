// SPDX-License-Identifier: MIT
use ash::vk;
use metal2vulkan_runtime::{
    Buffer, CommandBuffer, CommandBufferStatus, CommandQueue, ComputePipelineState, Device,
    DeviceOptions, MessageSeverity, MessageSink, PipelineOptions, ResourceOptions,
    ShaderCacheOptions, Size,
};
use serde_json::json;
use std::ffi::{CStr, c_char};
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod numerical {
    include!(env!("NMB_ACCEPTANCE_NUMERIC"));
}
const MATH: &[u8] = include_bytes!(concat!(env!("NMB_ACCEPTANCE_INPUTS"), "/native_maths.air"));
const HALF: &[u8] = include_bytes!(concat!(
    env!("NMB_ACCEPTANCE_INPUTS"),
    "/native_half_rounding.air"
));
const HALF_DIVISION: &[u8] = include_bytes!(concat!(
    env!("NMB_ACCEPTANCE_INPUTS"),
    "/native_half_division.air"
));
const EXAMPLES: &[u8] = include_bytes!(concat!(
    env!("NMB_ACCEPTANCE_INPUTS"),
    "/runtime_examples.air"
));
const FP32: &[u8] = include_bytes!(concat!(env!("NMB_ACCEPTANCE_INPUTS"), "/arith32.air"));
const FP16: &[u8] = include_bytes!(concat!(env!("NMB_ACCEPTANCE_INPUTS"), "/arith16.air"));
const SPV32: &[u8] = include_bytes!(concat!(env!("NMB_ACCEPTANCE_INPUTS"), "/arith32.spv"));
const SPV16: &[u8] = include_bytes!(concat!(env!("NMB_ACCEPTANCE_INPUTS"), "/arith16.spv"));
const SPV_SCALE: &[u8] = include_bytes!(concat!(env!("NMB_ACCEPTANCE_INPUTS"), "/scale.spv"));
const ITERATIONS: u64 = 256;
const WARM: usize = 2;
const SAMPLES: usize = 9;
const WAIT: Duration = Duration::from_secs(10);
type Result<T> = std::result::Result<T, String>;
fn err(value: impl std::fmt::Debug) -> String {
    format!("{value:?}")
}

fn emit(value: serde_json::Value) {
    println!("{value}");
}
fn stamp() -> serde_json::Value {
    json!({"boot":std::env::var("NATIVE_METAL_BOOT_SESSION").unwrap_or_default(),
        "component":env!("NMB_COMPONENT_ID"), "translator":env!("NMB_TRANSLATOR_ID"),
        "uptime_us":metal2vulkan_runtime::stage_trace::now_nanos()/1000})
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_acceptance_describe() {
    emit(json!({"schema":"unixnu-gpu-acceptance-1", "stamp":stamp(),
        "numeric":"upstream native_maths/half_rounding references and bounds, 4096 threads",
        "arithmetic":"FP32/FP16 vec4 independent multiply/add chains, 256 iterations, paired native SPIR-V on the same Vulkan device and queue",
        "bandwidth":"native Shared buffers, scale_and_offset and blit copy, paired native Vulkan dispatch/copy",
        "samples":SAMPLES,"warmups":WARM,"ratio_required":0.95,"runtime_llvm":false}));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_acceptance(
    loader: *const c_char,
    cache: *const c_char,
    mode: *const c_char,
) -> i32 {
    let run = std::panic::catch_unwind(|| {
        // The C launcher owns these NUL-terminated strings for this call.
        let (loader, cache, mode) = unsafe {
            (
                CStr::from_ptr(loader),
                CStr::from_ptr(cache),
                CStr::from_ptr(mode),
            )
        };
        execute(loader, cache, mode.to_str().map_err(err)?)
    });
    match run {
        Ok(Ok(pass)) => {
            emit(
                json!({"event":"complete","measurement_complete":true,"correct":pass,"stamp":stamp()}),
            );
            if pass { 0 } else { 1 }
        }
        Ok(Err(error)) => {
            emit(json!({"event":"error","error":error,"stamp":stamp()}));
            70
        }
        Err(_) => {
            emit(json!({"event":"error","error":"panic","stamp":stamp()}));
            70
        }
    }
}

fn execute(loader: &CStr, cache: &CStr, mode: &str) -> Result<bool> {
    nmb_acceptance_describe();
    let mut options = DeviceOptions::default();
    options.loader = Some(PathBuf::from(loader.to_string_lossy().into_owned()));
    options.shader_cache = ShaderCacheOptions::in_directory(cache.to_string_lossy().into_owned());
    if mode == "numeric" {
        options
            .layers
            .push(c"VK_LAYER_KHRONOS_validation".to_owned());
    }
    let validation_errors = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let errors = validation_errors.clone();
    options.messages = Some(MessageSink::new(move |m| {
        if m.id_name.ends_with("device-recovered") {
            fatal_gpu("Vulkan device recovered during acceptance");
        }
        if m.severity >= MessageSeverity::Error {
            errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        eprintln!("gpu-acceptance {} {}", m.id_name, m.text)
    }));
    let device = Device::system_default_with(&options).map_err(err)?;
    let p = device.properties();
    emit(
        json!({"event":"device","name":device.name(),"vendor_id":p.vendor_id,"device_id":p.device_id,
        "policy":format!("{:?}", device.compile_device().policy()),
        "profile":format!("{:?}", device.compile_device().profile()),"stamp":stamp()}),
    );
    if p.vendor_id != 0x5143 || p.device_id != 0x4305_0a01 {
        return Err("This calibration requires the sheng Adreno 740".into());
    }
    let queue = device.new_command_queue().map_err(err)?;
    let mut pass = true;
    if mode != "calibrate" {
        pass &= numeric(&device, &queue)?;
    }
    if mode != "numeric" {
        pass &= calibrate(&device, &queue)?;
    }
    let w = device.work_counts();
    emit(
        json!({"event":"runtime-counters","timestamp_slots":w.timestamp_slots_taken,
        "calibrations":w.calibrations,"calibration_disagreements":w.calibration_disagreements,"validation_errors":validation_errors.load(std::sync::atomic::Ordering::Relaxed),"stamp":stamp()}),
    );
    Ok(pass && validation_errors.load(std::sync::atomic::Ordering::Relaxed) == 0)
}

fn state(device: &Device, air: &[u8], name: &str) -> Result<ComputePipelineState> {
    let lib = device.new_library_with_compile_library(
        metal2vulkan::library::Library::from_air(air).map_err(err)?,
    );
    let f = lib.new_function_with_name(name).map_err(err)?;
    device
        .new_compute_pipeline_state_with_function(&f, &PipelineOptions::default())
        .map_err(err)
}
fn buffer(device: &Device, bytes: &[u8]) -> Result<Buffer> {
    device
        .new_buffer_with_bytes(bytes, ResourceOptions::shared())
        .map_err(err)
}
fn floats(buffer: &Buffer) -> Result<&[f32]> {
    let ptr = buffer.contents().ok_or("Shared buffer has no contents")?;
    // Every caller has waited for GPU completion and all lengths are multiples of four.
    Ok(unsafe { std::slice::from_raw_parts(ptr.as_ptr().cast(), buffer.length() as usize / 4) })
}
fn timed(cb: &CommandBuffer, device: &Device) -> Result<(f64, f64)> {
    let slots = device.work_counts().timestamp_slots_taken;
    let wall = Instant::now();
    cb.commit().map_err(err)?;
    if cb.wait_until_completed_for(WAIT).is_err() {
        fatal_gpu("runtime completion exceeded 10 seconds");
    }
    if cb.status() != CommandBufferStatus::Completed {
        return Err(format!(
            "command buffer {:?}: {:?}",
            cb.status(),
            cb.error()
        ));
    }
    let us = (cb.gpu_end_time().map_err(err)? - cb.gpu_start_time().map_err(err)?) * 1e6;
    if us <= 0.0 || !us.is_finite() {
        return Err("GPU timestamp interval unavailable".into());
    }
    // The runtime allocates timestamp slots while encoding, before commit. The physical queue
    // timestamp support is checked by Native::new; the total proves timing was in use.
    if slots == 0 {
        return Err("Runtime supplied no GPU timestamp slots".into());
    }
    Ok((us, wall.elapsed().as_secs_f64() * 1e6))
}
fn dispatch(
    device: &Device,
    queue: &CommandQueue,
    state: &ComputePipelineState,
    buffers: &[&Buffer],
    count: u64,
) -> Result<(f64, f64)> {
    let cb = queue.command_buffer().map_err(err)?;
    let mut enc = cb.compute_command_encoder().map_err(err)?;
    enc.set_compute_pipeline_state(state).map_err(err)?;
    let raw = unsafe { device.raw() };
    let limits = unsafe {
        raw.instance
            .get_physical_device_properties(raw.physical_device)
    }
    .limits;
    let chunk = limits.max_compute_work_group_count[0] as u64 * 64;
    let stride = buffers[0].length() / count;
    for first in (0..count).step_by(chunk as usize) {
        let units = (count - first).min(chunk);
        for (i, b) in buffers.iter().enumerate() {
            enc.set_buffer(Some(b), if i < 2 { first * stride } else { 0 }, i as u32)
                .map_err(err)?;
        }
        enc.dispatch_threadgroups(Size::new(units / 64, 1, 1), Size::new(64, 1, 1))
            .map_err(err)?;
    }
    enc.end_encoding().map_err(err)?;
    timed(&cb, device)
}
fn copy(
    device: &Device,
    queue: &CommandQueue,
    input: &Buffer,
    output: &Buffer,
) -> Result<(f64, f64)> {
    let cb = queue.command_buffer().map_err(err)?;
    let mut enc = cb.blit_command_encoder().map_err(err)?;
    enc.copy_from_buffer_to_buffer(input, 0, output, 0, input.length())
        .map_err(err)?;
    enc.end_encoding().map_err(err)?;
    timed(&cb, device)
}
fn numeric(device: &Device, queue: &CommandQueue) -> Result<bool> {
    let inputs = numerical::operands();
    let bytes: Vec<u8> = inputs
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let input = buffer(device, &bytes)?;
    let output = device
        .new_buffer(4096 * 14 * 4, ResourceOptions::shared())
        .map_err(err)?;
    emit(json!({"event":"begin","case":"native-maths","stamp":stamp()}));
    let pipeline = state(device, MATH, "nm")?;
    dispatch(device, queue, &pipeline, &[&input, &output], 4096)?;
    let results = floats(&output)?;
    let mut failures = 0;
    let mut examples = Vec::new();
    for (x, r) in inputs.iter().zip(results.as_chunks::<14>().0) {
        for problem in numerical::broken(x, r, true) {
            failures += 1;
            if examples.len() < 12 {
                examples.push(problem);
            }
        }
    }
    emit(
        json!({"event":"numeric","case":"native-maths","words":4096*14,"failures":failures,"examples":examples,"stamp":stamp()}),
    );
    let mut half_failures = 0;
    let inputs = numerical::half_operands();
    let bytes: Vec<u8> = inputs
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let input = buffer(device, &bytes)?;
    let output = device
        .new_buffer(4096 * 2 * 4, ResourceOptions::shared())
        .map_err(err)?;
    emit(json!({"event":"begin","case":"native-half-rounding","stamp":stamp()}));
    let pipeline = state(device, HALF, "hr")?;
    dispatch(device, queue, &pipeline, &[&input, &output], 4096)?;
    for (&[a, b], r) in inputs.iter().zip(floats(&output)?.as_chunks::<2>().0) {
        // Bounds and allowed dropped-rounding results are the upstream half_rounding lane's.
        let ha = numerical::metal_half(a as f64);
        let hb = numerical::metal_half(b as f64);
        let sum = numerical::metal_half(ha + hb);
        let allowed = [
            a + b,
            ha as f32 + hb as f32,
            numerical::metal_half((a + b) as f64) as f32,
        ];
        if !(r[0].to_bits() == (sum as f32).to_bits()
            || numerical::half_within_sum(r[0], a as f64, b as f64)
            || allowed.contains(&r[0]))
        {
            half_failures += 1;
        }
        if !(r[1].to_bits() == (ha as f32).to_bits()
            || numerical::half_within(r[1], a as f64)
            || r[1].to_bits() == a.to_bits())
        {
            half_failures += 1;
        }
    }
    emit(
        json!({"event":"numeric","case":"native-half-rounding","words":4096*2,"failures":half_failures,"stamp":stamp()}),
    );
    // These normal, exactly representable operands cover the native16 divisor limit.
    // The large divisors need normalization before a half reciprocal can stay normal.
    let cases = [
        [65504.0f32, 65504.0],
        [65504.0, 32768.0],
        [32768.0, 65504.0],
        [16384.0, 16384.0],
        [16392.0, 16392.0],
        [-65504.0, 65504.0],
        [65504.0, -32768.0],
        [1.0, 1.0],
        [0.00006103515625, 0.00006103515625],
    ];
    let inputs: Vec<[f32; 2]> = (0..4096).map(|i| cases[i % cases.len()]).collect();
    let bytes: Vec<u8> = inputs
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let input = buffer(device, &bytes)?;
    let output = device
        .new_buffer(4096 * 2 * 4, ResourceOptions::shared())
        .map_err(err)?;
    emit(json!({"event":"begin","case":"native-half-division-range","stamp":stamp()}));
    let pipeline = state(device, HALF_DIVISION, "hr")?;
    dispatch(device, queue, &pipeline, &[&input, &output], 4096)?;
    let mut division_failures = 0;
    let mut max_error_ulp = 0.0f64;
    for (&[a, b], r) in inputs.iter().zip(floats(&output)?.as_chunks::<2>().0) {
        let want = a as f64 / b as f64;
        let error = (r[0] as f64 - want).abs() / numerical::half_ulp(want);
        if !error.is_finite() || error > 2.5 {
            division_failures += 1;
        }
        max_error_ulp = max_error_ulp.max(error);
    }
    emit(
        json!({"event":"numeric","case":"native-half-division-range","words":4096,
        "failures":division_failures,"max_error_half_ulp":max_error_ulp,"bound_half_ulp":2.5,"stamp":stamp()}),
    );
    Ok(failures == 0 && half_failures == 0 && division_failures == 0)
}
fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}
fn paired(
    mut translated: impl FnMut() -> Result<(f64, f64)>,
    mut native: impl FnMut() -> Result<(f64, f64)>,
) -> Result<(f64, f64, f64, f64)> {
    let (mut t, mut n, mut tw, mut nw) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for i in 0..WARM + SAMPLES {
        let (a, b) = if i % 2 == 0 {
            (translated()?, native()?)
        } else {
            let b = native()?;
            (translated()?, b)
        };
        if i >= WARM {
            t.push(a.0);
            tw.push(a.1);
            n.push(b.0);
            nw.push(b.1);
        }
    }
    Ok((
        median(&mut t),
        median(&mut n),
        median(&mut tw),
        median(&mut nw),
    ))
}
fn row(
    kind: &str,
    precision: &str,
    bytes: u64,
    operations: u64,
    times: (f64, f64, f64, f64),
    difference: f64,
) -> bool {
    let (t, n, tw, nw) = times;
    let ratio = n / t;
    let limit = match (kind, precision) {
        ("arithmetic", "float16") => 0.5,
        ("arithmetic", _) => 1e-4,
        _ => 0.0,
    };
    let correct = difference.is_finite() && difference <= limit;
    let target_met = ratio >= 0.95;
    emit(
        json!({"event":"calibration","kind":kind,"precision":precision,"bytes":bytes,"operations":operations,
        "translator_gpu_us":t,"native_gpu_us":n,"translator_wall_us":tw,"native_wall_us":nw,
        "native_fraction":ratio,"required_fraction":0.95,"translator_gflops":operations as f64/t/1000.0,
        "translator_gbps":bytes as f64/t/1000.0,"max_output_difference":difference,"comparison_limit":limit,"correct":correct,"target_met":target_met,"stamp":stamp()}),
    );
    correct
}
fn calibrate(device: &Device, queue: &CommandQueue) -> Result<bool> {
    let mut pass = true;
    for (name, air, spv) in [("float32", FP32, SPV32), ("float16", FP16, SPV16)] {
        if name == "float16"
            && !device
                .compile_device()
                .profile()
                .enabled()
                .features
                .contains(&metal2vulkan::device::VkFeature::ShaderFloat16)
        {
            emit(
                json!({"event":"unsupported","case":"float16","reason":"shaderFloat16 is not enabled","stamp":stamp()}),
            );
            pass = false;
            continue;
        }
        emit(json!({"event":"begin","case":"arithmetic","precision":name,"stamp":stamp()}));
        let pipeline = state(device, air, "arith")?;
        for count in [131072u64, 524288] {
            let data: Vec<u8> = (0..count * 4)
                .flat_map(|i| ((i % 1024) as f32 / 2048.0).to_le_bytes())
                .collect();
            let input = buffer(device, &data)?;
            let output = device
                .new_buffer(data.len() as u64, ResourceOptions::shared())
                .map_err(err)?;
            let control = device
                .new_buffer(data.len() as u64, ResourceOptions::shared())
                .map_err(err)?;
            let native = Native::new(device, queue, spv, &[&input, &control], count, 16)?;
            let times = paired(
                || dispatch(device, queue, &pipeline, &[&input, &output], count),
                || native.run(queue, false),
            )?;
            let difference =
                floats(&output)?
                    .iter()
                    .zip(floats(&control)?)
                    .fold(0.0f64, |v, (a, b)| {
                        if a.is_finite() && b.is_finite() {
                            v.max((*a as f64 - *b as f64).abs())
                        } else {
                            f64::INFINITY
                        }
                    });
            pass &= row(
                "arithmetic",
                name,
                0,
                count * 4 * ITERATIONS * 2,
                times,
                difference,
            );
        }
    }
    let pipeline = state(device, EXAMPLES, "scale_and_offset")?;
    let scale = buffer(device, &2.0f32.to_le_bytes())?;
    for mib in [16u64, 64, 256] {
        emit(json!({"event":"begin","case":"shared-bandwidth","mib":mib,"stamp":stamp()}));
        let length = mib << 20;
        let count = length / 4;
        let input = device
            .new_buffer(length, ResourceOptions::shared())
            .map_err(err)?;
        if let Some(at) = input.contents() {
            unsafe {
                std::ptr::write_bytes(at.as_ptr(), 0x3e, length as usize);
            }
        }
        let output = device
            .new_buffer(length, ResourceOptions::shared())
            .map_err(err)?;
        let control = device
            .new_buffer(length, ResourceOptions::shared())
            .map_err(err)?;
        let native = Native::new(
            device,
            queue,
            SPV_SCALE,
            &[&input, &control, &scale],
            count,
            4,
        )?;
        let times = paired(
            || dispatch(device, queue, &pipeline, &[&input, &output, &scale], count),
            || native.run(queue, false),
        )?;
        let difference = floats(&output)?
            .iter()
            .zip(floats(&control)?)
            .fold(0.0f64, |v, (a, b)| v.max((*a as f64 - *b as f64).abs()));
        pass &= row(
            "shared-kernel-read-write",
            "float32",
            length * 2,
            0,
            times,
            difference,
        );
        let times = paired(
            || copy(device, queue, &input, &output),
            || native.run(queue, true),
        )?;
        let difference = floats(&output)?
            .iter()
            .zip(floats(&control)?)
            .fold(0.0f64, |v, (a, b)| v.max((*a as f64 - *b as f64).abs()));
        pass &= row(
            "shared-blit-copy",
            "bytes",
            length * 2,
            0,
            times,
            difference,
        );
    }
    Ok(pass)
}

fn fatal_gpu(why: &str) -> ! {
    emit(json!({"event":"gpu-timeout","error":why,"reboot_required":true,"stamp":stamp()}));
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // A timed-out queue may still own every resource; exit without running Vulkan destructors.
    unsafe { libc::_exit(75) }
}

struct Native<'a> {
    device: &'a Device,
    owner: vk::Device,
    pool: vk::CommandPool,
    commands: [vk::CommandBuffer; 2],
    queries: vk::QueryPool,
    fence: vk::Fence,
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    descriptors: vk::DescriptorPool,
    pipeline: vk::Pipeline,
    module: vk::ShaderModule,
    period: f64,
    bits: u32,
}
impl<'a> Native<'a> {
    fn new(
        device: &'a Device,
        queue: &CommandQueue,
        spv: &[u8],
        buffers: &[&Buffer],
        count: u64,
        stride: u64,
    ) -> Result<Self> {
        // All borrowed objects outlive native submissions, which finish before this object drops.
        unsafe {
            let raw = device.raw();
            let vk = raw.device;
            let owner = vk.handle();
            let family = queue.with_vk_queue(|d, _, f| {
                if d.handle() != owner {
                    fatal_gpu("Vulkan device changed before native setup");
                }
                f
            });
            let families = raw
                .instance
                .get_physical_device_queue_family_properties(raw.physical_device);
            let bits = families[family as usize].timestamp_valid_bits;
            if bits == 0 {
                return Err("Native calibration requires hardware timestamps".into());
            }
            let limits = raw
                .instance
                .get_physical_device_properties(raw.physical_device)
                .limits;
            let period = limits.timestamp_period as f64;
            let chunk = limits.max_compute_work_group_count[0] as u64 * 64;
            let pieces = count.div_ceil(chunk) as usize;
            let pool = vk
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(family),
                    None,
                )
                .map_err(err)?;
            let cmds = device
                .with_loader_word(|| {
                    vk.allocate_command_buffers(
                        &vk::CommandBufferAllocateInfo::default()
                            .command_pool(pool)
                            .level(vk::CommandBufferLevel::PRIMARY)
                            .command_buffer_count(2),
                    )
                })
                .map_err(err)?;
            let commands = [cmds[0], cmds[1]];
            let queries = vk
                .create_query_pool(
                    &vk::QueryPoolCreateInfo::default()
                        .query_type(vk::QueryType::TIMESTAMP)
                        .query_count(2),
                    None,
                )
                .map_err(err)?;
            let fence = vk
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .map_err(err)?;
            let bindings: Vec<_> = (0..buffers.len())
                .map(|i| {
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(i as u32)
                        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                        .descriptor_count(1)
                        .stage_flags(vk::ShaderStageFlags::COMPUTE)
                })
                .collect();
            let set_layout = vk
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(err)?;
            let layout = vk
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&[set_layout]),
                    None,
                )
                .map_err(err)?;
            let sizes = [vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: (buffers.len() * pieces) as u32,
            }];
            let descriptors = vk
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(pieces as u32)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(err)?;
            let layouts = vec![set_layout; pieces];
            let sets = vk
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(descriptors)
                        .set_layouts(&layouts),
                )
                .map_err(err)?;
            for (piece, &set) in sets.iter().enumerate() {
                let first = piece as u64 * chunk;
                let units = (count - first).min(chunk);
                let infos: Vec<_> = buffers
                    .iter()
                    .enumerate()
                    .map(|(i, b)| {
                        let (buffer, offset) = b.vk_buffer();
                        [vk::DescriptorBufferInfo::default()
                            .buffer(buffer)
                            .offset(offset + if i < 2 { first * stride } else { 0 })
                            .range(if i < 2 { units * stride } else { b.length() })]
                    })
                    .collect();
                let writes: Vec<_> = infos
                    .iter()
                    .enumerate()
                    .map(|(i, b)| {
                        vk::WriteDescriptorSet::default()
                            .dst_set(set)
                            .dst_binding(i as u32)
                            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                            .buffer_info(b)
                    })
                    .collect();
                vk.update_descriptor_sets(&writes, &[]);
            }
            let words: Vec<u32> = spv
                .chunks_exact(4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .collect();
            let module = vk
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)
                .map_err(err)?;
            let stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(module)
                .name(c"main");
            let pipeline = vk
                .create_compute_pipelines(
                    vk::PipelineCache::null(),
                    &[vk::ComputePipelineCreateInfo::default()
                        .stage(stage)
                        .layout(layout)],
                    None,
                )
                .map_err(|(_, e)| err(e))?[0];
            let barriers = [vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::MEMORY_WRITE | vk::AccessFlags::HOST_WRITE)
                .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)];
            for (i, &cb) in commands.iter().enumerate() {
                vk.begin_command_buffer(cb, &vk::CommandBufferBeginInfo::default())
                    .map_err(err)?;
                vk.cmd_reset_query_pool(cb, queries, 0, 2);
                vk.cmd_pipeline_barrier(
                    cb,
                    vk::PipelineStageFlags::ALL_COMMANDS | vk::PipelineStageFlags::HOST,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::DependencyFlags::empty(),
                    &barriers,
                    &[],
                    &[],
                );
                vk.cmd_write_timestamp(cb, vk::PipelineStageFlags::BOTTOM_OF_PIPE, queries, 0);
                if i == 0 {
                    vk.cmd_bind_pipeline(cb, vk::PipelineBindPoint::COMPUTE, pipeline);
                    for (piece, &set) in sets.iter().enumerate() {
                        vk.cmd_bind_descriptor_sets(
                            cb,
                            vk::PipelineBindPoint::COMPUTE,
                            layout,
                            0,
                            &[set],
                            &[],
                        );
                        let units = (count - piece as u64 * chunk).min(chunk);
                        vk.cmd_dispatch(cb, (units / 64) as u32, 1, 1);
                    }
                } else {
                    let (a, ao) = buffers[0].vk_buffer();
                    let (b, bo) = buffers[1].vk_buffer();
                    vk.cmd_copy_buffer(
                        cb,
                        a,
                        b,
                        &[vk::BufferCopy {
                            src_offset: ao,
                            dst_offset: bo,
                            size: buffers[0].length(),
                        }],
                    );
                }
                vk.cmd_pipeline_barrier(
                    cb,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::PipelineStageFlags::HOST,
                    vk::DependencyFlags::empty(),
                    &[vk::MemoryBarrier::default()
                        .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
                        .dst_access_mask(vk::AccessFlags::HOST_READ)],
                    &[],
                    &[],
                );
                vk.cmd_write_timestamp(cb, vk::PipelineStageFlags::BOTTOM_OF_PIPE, queries, 1);
                vk.end_command_buffer(cb).map_err(err)?;
            }
            Ok(Self {
                device,
                owner,
                pool,
                commands,
                queries,
                fence,
                layout,
                set_layout,
                descriptors,
                pipeline,
                module,
                period,
                bits,
            })
        }
    }
    fn run(&self, queue: &CommandQueue, copy: bool) -> Result<(f64, f64)> {
        let wall = Instant::now();
        unsafe {
            let raw = self.device.raw();
            let vk = raw.device;
            if vk.handle() != self.owner {
                fatal_gpu("Vulkan device changed during native calibration");
            }
            vk.reset_fences(&[self.fence]).map_err(err)?;
            queue
                .with_vk_queue(|d, q, _| {
                    if d.handle() != self.owner {
                        fatal_gpu("Vulkan queue changed during native calibration");
                    }
                    d.queue_submit(
                        q,
                        &[vk::SubmitInfo::default()
                            .command_buffers(&[self.commands[usize::from(copy)]])],
                        self.fence,
                    )
                })
                .map_err(err)?;
            match vk.wait_for_fences(&[self.fence], true, 10_000_000_000) {
                Err(vk::Result::TIMEOUT) => fatal_gpu("native completion exceeded 10 seconds"),
                other => other.map_err(err)?,
            }
            let mut times = [0u64; 2];
            vk.get_query_pool_results(self.queries, 0, &mut times, vk::QueryResultFlags::TYPE_64)
                .map_err(err)?;
            let mask = if self.bits == 64 {
                u64::MAX
            } else {
                (1u64 << self.bits) - 1
            };
            let us = (times[1].wrapping_sub(times[0]) & mask) as f64 * self.period / 1000.0;
            if us <= 0.0 {
                return Err("Native timestamp interval is zero".into());
            }
            Ok((us, wall.elapsed().as_secs_f64() * 1e6))
        }
    }
}
impl Drop for Native<'_> {
    fn drop(&mut self) {
        unsafe {
            let raw = self.device.raw();
            let vk = raw.device;
            if vk.handle() != self.owner {
                fatal_gpu("Vulkan device changed during native calibration");
            }
            vk.destroy_pipeline(self.pipeline, None);
            vk.destroy_shader_module(self.module, None);
            vk.destroy_descriptor_pool(self.descriptors, None);
            vk.destroy_pipeline_layout(self.layout, None);
            vk.destroy_descriptor_set_layout(self.set_layout, None);
            vk.destroy_fence(self.fence, None);
            vk.destroy_query_pool(self.queries, None);
            vk.destroy_command_pool(self.pool, None);
        }
    }
}
