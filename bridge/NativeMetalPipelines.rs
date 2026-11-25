// SPDX-License-Identifier: MIT
//! Consumer-owned sampler indices accompany public upstream pipeline handles.
use metal2vulkan::pipeline::{
    ComputePipelineDescriptor, NumberedSampler, RenderPipelineDescriptor,
};
use metal2vulkan_runtime as rt;

fn sampler_ids(
    device: &rt::Device,
    samplers: &[NumberedSampler],
    held: &mut Vec<crate::ownership::Reservation>,
    refused: &mut Option<String>,
) -> Result<Vec<u32>, rt::Error> {
    let mut ids = Vec::with_capacity(samplers.len());
    for sampler in samplers {
        match sampler {
            NumberedSampler::Read | NumberedSampler::Write => ids.push(0),
            NumberedSampler::Constexpr(_) => {
                match crate::ownership::reserve(device, rt::IndexClass::Sampler).ok() {
                    Some(index) => {
                        ids.push(index.index());
                        held.push(index);
                    }
                    None => {
                        *refused = Some("pipeline sampler index space exhausted".into());
                        break;
                    }
                }
            }
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
) -> Result<ComputePipelineState, String> {
    crate::archive::Registry::for_device(device)?.prepare_compute(device, descriptor);
    let mut held = Vec::new();
    let mut refused = None;
    let raw = device
        .new_compute_pipeline_state(descriptor, |samplers| {
            sampler_ids(device, samplers, &mut held, &mut refused)
        })
        .map_err(|e| refused.unwrap_or_else(|| format!("compute pipeline: {e:?}")))?;
    raw.finish_pipeline()
        .map_err(|e| format!("compute pipeline finish: {e:?}"))?;
    Ok(ComputePipelineState {
        raw,
        ownership: crate::ownership::Ownership::new(held.iter().map(|r| r.owner()).collect()),
    })
}
pub fn render(
    device: &rt::Device,
    descriptor: &RenderPipelineDescriptor,
) -> Result<RenderPipelineState, String> {
    crate::archive::Registry::for_device(device)?.prepare_render(device, descriptor);
    let mut held = Vec::new();
    let mut refused = None;
    let raw = device
        .new_render_pipeline_state(descriptor, |samplers| {
            sampler_ids(device, samplers, &mut held, &mut refused)
        })
        .map_err(|e| refused.unwrap_or_else(|| format!("render pipeline: {e:?}")))?;
    raw.finish_pipeline()
        .map_err(|e| format!("render pipeline finish: {e:?}"))?;
    Ok(RenderPipelineState {
        raw,
        ownership: crate::ownership::Ownership::new(held.iter().map(|r| r.owner()).collect()),
    })
}

#[derive(Clone)]
pub struct ComputePipelineState {
    raw: rt::ComputePipelineState,
    pub(crate) ownership: std::sync::Arc<crate::ownership::Ownership>,
}
impl std::ops::Deref for ComputePipelineState {
    type Target = rt::ComputePipelineState;
    fn deref(&self) -> &Self::Target {
        &self.raw
    }
}
#[derive(Clone)]
pub struct RenderPipelineState {
    raw: rt::RenderPipelineState,
    pub(crate) ownership: std::sync::Arc<crate::ownership::Ownership>,
}
impl std::ops::Deref for RenderPipelineState {
    type Target = rt::RenderPipelineState;
    fn deref(&self) -> &Self::Target {
        &self.raw
    }
}
