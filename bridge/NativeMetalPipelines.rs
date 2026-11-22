// SPDX-License-Identifier: MIT
//! Pipeline sampler indices are reserved before creation and owned by the runtime states afterward.
use metal2vulkan::pipeline::{
    ComputePipelineDescriptor, NumberedSampler, RenderPipelineDescriptor,
};
use metal2vulkan_runtime as rt;

fn sampler_ids(
    device: &rt::Device,
    samplers: &[NumberedSampler],
    held: &mut Vec<rt::IndexReservation>,
    refused: &mut Option<String>,
) -> Result<Vec<u32>, rt::Error> {
    let mut ids = Vec::with_capacity(samplers.len());
    for sampler in samplers {
        match sampler {
            NumberedSampler::Read | NumberedSampler::Write => ids.push(0),
            NumberedSampler::Constexpr(_) => match device.reserve_index(rt::IndexClass::Sampler) {
                Some(index) => {
                    ids.push(index.index());
                    held.push(index);
                }
                None => {
                    *refused = Some("pipeline sampler index space exhausted".into());
                    break;
                }
            },
            _ => {
                *refused = Some("unknown upstream pipeline sampler kind".into());
                break;
            }
        }
    }
    if refused.is_some() {
        return Err(rt::Error::Refused(rt::Refusal::SamplerIds {
            numbered: samplers.len(),
            given: ids.len(),
        }));
    }
    Ok(ids)
}
pub fn compute(
    device: &rt::Device,
    descriptor: &ComputePipelineDescriptor,
) -> Result<rt::ComputePipelineState, String> {
    let mut held = Vec::new();
    let mut refused = None;
    let state = device.new_compute_pipeline_state(descriptor, |samplers| {
        sampler_ids(device, samplers, &mut held, &mut refused)
    });
    state.map_err(|e| refused.unwrap_or_else(|| format!("compute pipeline: {e:?}")))
}
pub fn render(
    device: &rt::Device,
    descriptor: &RenderPipelineDescriptor,
) -> Result<rt::RenderPipelineState, String> {
    let mut held = Vec::new();
    let mut refused = None;
    let state = device.new_render_pipeline_state(descriptor, |samplers| {
        sampler_ids(device, samplers, &mut held, &mut refused)
    });
    state.map_err(|e| refused.unwrap_or_else(|| format!("render pipeline: {e:?}")))
}
