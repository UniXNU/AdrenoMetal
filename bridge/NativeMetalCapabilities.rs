// SPDX-License-Identifier: MIT
//! Metal capability answers from the runtime floor and translator's enabled profile.
use metal2vulkan::device::OpBits;
use metal2vulkan::metal::PixelFormat;
use metal2vulkan_runtime::Device;

pub const PRIVATE_COUNTS: &[&str] = &[
    "gpuCoreCount",
    "maxVertexAmplificationCount",
    "deviceLinearTextureAlignmentBytes",
    "linearTextureAlignmentBytes",
    "iosurfaceTextureAlignmentBytes",
    "iosurfaceReadOnlyTextureAlignmentBytes",
    "maxVertexBuffers",
    "maxFragmentBuffers",
    "maxComputeBuffers",
    "maxTileBuffers",
    "maxVertexTextures",
    "maxFragmentTextures",
    "maxComputeTextures",
    "maxTileTextures",
    "maxVertexSamplers",
    "maxFragmentSamplers",
    "maxComputeSamplers",
    "maxTileSamplers",
    "maxComputeLocalMemorySizes",
    "maxComputeThreadgroupMemory",
    "maxComputeThreadgroupMemoryAlignmentBytes",
    "maxPredicatedNestingDepth",
    "maxRasterizationRateLayerCount",
    "maxTextureWidth2D",
    "maxTextureHeight2D",
    "maxTextureLayers",
    "maxTotalComputeThreadsPerThreadgroup",
    "maxViewportCount",
];
pub const PRIVATE_FLAGS: &[&str] = &[
    "isFramebufferReadSupported",
    "isLargeMRTSupported",
    "isRGB10A2GammaSupported",
    "supportsBufferWithAddressRanges",
    "supportsBufferWithIOSurface",
    "supportsCommandBufferJump",
    "supportsComputeCompressedTextureWrite",
    "supportsConditionalLoadStore",
    "supportsDynamicAttributeStride",
    "supportsExtendedXR10Formats",
    "supportsFloat16BCubicFiltering",
    "supportsHeapWithAddressRanges",
    "supportsImageBlocks",
    "supportsMemoryOrderAtomics",
    "supportsNativeHardwareFP16",
    "supportsNonSquareTileShaders",
    "supportsProgrammableSamplePositions",
    "supportsResourceDetachBacking",
    "supportsSharedTextureHandles",
    "supportsSIMDReduction",
    "supportsSIMDShuffleAndFill",
    "supportsTexture2DMultisampleArray",
    "supportsTileShaders",
];

pub fn answer(device: &Device, selector: &str, arg: u64) -> Result<u64, String> {
    let translator = device.translator();
    let p = translator.metal_properties();
    let profile = translator.profile();
    let l = profile.limits();
    let unsupported = || format!("runtime v2 has no device answer for {selector}");
    let narrow = |v| u32::try_from(v).map_err(|_| format!("device query argument {v} exceeds u32"));
    Ok(match selector {
        "gpuCoreCount" => 1,
        "maxVertexAmplificationCount" => 1,
        "maxThreadgroupMemoryLength" | "maxComputeThreadgroupMemory" => {
            device.max_threadgroup_memory_length() as u64
        }
        "argumentBuffersSupport" => device.argument_buffers_support().as_raw() as u64,
        "readWriteTextureSupport" => device.read_write_texture_support().as_raw() as u64,
        "maxColorAttachments" => l
            .max_color_attachments
            .min(l.max_fragment_output_attachments)
            .min(8) as u64,
        "minConstantBufferAlignmentBytes" => 4,
        "minimumLinearTextureAlignmentForPixelFormat:" => device
            .minimum_linear_texture_alignment(
                PixelFormat::try_from(narrow(arg)?).map_err(|_| "invalid pixel format")?,
            )
            .map_err(|e| format!("linear alignment: {e:?}"))?,
        "deviceLinearReadOnlyTextureAlignmentBytes"
        | "deviceLinearTextureAlignmentBytes"
        | "linearTextureAlignmentBytes"
        | "iosurfaceTextureAlignmentBytes"
        | "iosurfaceReadOnlyTextureAlignmentBytes" => [
            PixelFormat::BGRA8Unorm,
            PixelFormat::RGBA8Unorm,
            PixelFormat::RGBA16Float,
        ]
        .into_iter()
        .try_fold(16, |a, pf| {
            device
                .minimum_linear_texture_alignment(pf)
                .map(|b| a.max(b))
                .map_err(|e| format!("linear alignment: {e:?}"))
        })?,
        "maxVertexBuffers"
        | "maxFragmentBuffers"
        | "maxComputeBuffers"
        | "maxComputeLocalMemorySizes" => 31,
        "maxVertexTextures" | "maxFragmentTextures" | "maxComputeTextures" => 128,
        "maxVertexSamplers" | "maxFragmentSamplers" | "maxComputeSamplers" => 16,
        "maxTileBuffers"
        | "maxTileTextures"
        | "maxTileSamplers"
        | "maxRasterizationRateLayerCount"
        | "maxPredicatedNestingDepth" => 0,
        "maxComputeThreadgroupMemoryAlignmentBytes" => 16,
        "maxTextureWidth2D" | "maxTextureHeight2D" => p.max_texture_dimension_2d as u64,
        "maxTextureLayers" => l.max_image_array_layers.min(2048) as u64,
        "maxTotalComputeThreadsPerThreadgroup" => p.max_total_threads_per_threadgroup as u64,
        "maxViewportCount" => {
            let raw = device.raw();
            // SAFETY: the runtime holds both handles for this call.
            unsafe {
                raw.instance
                    .get_physical_device_properties(raw.physical_device)
            }
            .limits
            .max_viewports
            .min(16) as u64
        }
        "areRasterOrderGroupsSupported" | "supportsRasterOrderGroups" => {
            profile.fragment().pixel_interlock as u64
        }
        "isDepth24Stencil8PixelFormatSupported" => p.supports_depth24_stencil8 as u64,
        "supports32BitFloatFiltering" => p.supports_32bit_float_filtering as u64,
        "supportsNativeHardwareFP16" => profile
            .enabled()
            .features
            .contains(&metal2vulkan::device::VkFeature::ShaderFloat16)
            as u64,
        "supportsTextureSampleCount:" => device.supports_texture_sample_count(narrow(arg)?) as u64,
        "supportsTexture2DMultisampleArray" => [2, 4]
            .into_iter()
            .any(|n| device.supports_texture_sample_count(n))
            as u64,
        "isLargeMRTSupported" => {
            (l.max_color_attachments
                .min(l.max_fragment_output_attachments)
                >= 8) as u64
        }
        "supportsSamplerReductionMode" => profile
            .enabled()
            .features
            .contains(&metal2vulkan::device::VkFeature::SamplerFilterMinmax)
            as u64,
        "supportsSIMDGroup" => (p.simd.kernel.cross_lane != OpBits::NONE) as u64,
        "supportsSIMDReduction" => p.simd.kernel.cross_lane.contains(OpBits::ARITHMETIC) as u64,
        "supportsSIMDShuffleAndFill" => (p.simd.kernel.cross_lane.contains(OpBits::SHUFFLE)
            && p.simd.kernel.cross_lane.contains(OpBits::SHUFFLE_RELATIVE))
            as u64,
        "supportsPrimitiveType:" => (arg <= 4) as u64,
        "supportsTextureWriteRoundingMode:" => {
            metal2vulkan::metal::TextureWriteRoundingMode::from_raw(arg).is_ok() as u64
        }
        "supportsVertexAmplificationCount:" => (arg == 1) as u64,
        // Keep the frontend's existing OS compatibility families. No higher family is inferred from the Vulkan floor.
        "supportsFamily:" => [1001, 1002, 1003, 3001, 3002].contains(&arg) as u64,
        "isFramebufferReadSupported" => 1,
        "isRGB10A2GammaSupported"
        | "supportsMemorylessRenderTargets"
        | "supportsTileShaders"
        | "supportsImageBlocks"
        | "supportsNonSquareTileShaders"
        | "supportsYCBCRFormats12"
        | "supportsYCBCRPackedFormats12"
        | "supportsExtendedXR10Formats"
        | "supportsFloat16BCubicFiltering"
        | "areProgrammableSamplePositionsSupported"
        | "supportsProgrammableSamplePositions"
        | "supportsRasterizationRateMapWithLayerCount:"
        | "supportsCommandBufferJump"
        | "supportsComputeCompressedTextureWrite"
        | "supportsConditionalLoadStore"
        | "supportsDynamicAttributeStride"
        | "supportsMemoryOrderAtomics"
        | "supportsResourceDetachBacking"
        | "supportsSharedTextureHandles"
        | "supportsBufferWithAddressRanges"
        | "supportsBufferWithIOSurface"
        | "supportsHeapWithAddressRanges" => 0,
        _ => return Err(unsupported()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_selector_has_a_match_arm_in_answer() {
        let source = include_str!("NativeMetalCapabilities.rs");
        let answer_body = source
            .split("pub fn answer(")
            .nth(1)
            .expect("answer function body");
        let answer_body = answer_body
            .split("#[cfg(test)]")
            .next()
            .expect("answer before tests");
        let mut missing = Vec::new();
        for selector in PRIVATE_COUNTS
            .iter()
            .chain(PRIVATE_FLAGS.iter())
            .copied()
            .chain(crate::objects::PUBLIC_ANSWERS.iter().map(|(s, _)| *s))
        {
            let needle = format!("\"{selector}\"");
            if !answer_body.contains(&needle) {
                missing.push(selector);
            }
        }
        assert!(
            missing.is_empty(),
            "registered selectors missing from capabilities::answer: {missing:?}"
        );
    }
}
