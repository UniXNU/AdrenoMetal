/* SPDX-License-Identifier: MIT */
/* C ABI of the bridge's Metal objects (NativeMetalObjects.rs): the metal2vulkan runtime's device, resources, states, queues, command buffers
 * and encoders as opaque handles, for the Objective-C frontend (NativeBridgeMetalFrontend.mm). Enumerations and option words are Metal's raw
 * values. A call that fails returns 0/NULL and leaves its reason in nmb_last_error() (per thread). Every handle is released once with its
 * release call; an encoder is released by its end call (or by nmb_encoder_discard when it is never ended). */
#ifndef NATIVE_METAL_OBJECTS_H
#define NATIVE_METAL_OBJECTS_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct NmbDevice NmbDevice;
typedef struct NmbBuffer NmbBuffer;
typedef struct NmbHeap NmbHeap;
typedef struct NmbTexture NmbTexture;
typedef struct NmbLibrary NmbLibrary;
typedef struct NmbFunction NmbFunction;
typedef struct NmbComputePipeline NmbComputePipeline;
typedef struct NmbRenderPipeline NmbRenderPipeline;
typedef struct NmbSampler NmbSampler;
typedef struct NmbDepthStencil NmbDepthStencil;
typedef struct NmbQueue NmbQueue;
typedef struct NmbCommandBuffer NmbCommandBuffer;
typedef struct NmbEncoder NmbEncoder;
typedef struct NmbEvent NmbEvent;
typedef struct NmbFence NmbFence;

const char *nmb_last_error(void);
/* The bridge's embedded AIR fixtures: the runtime's own runtime_examples and kernel_function_constant_gate_fold. */
const void *nmb_fixture_air(size_t *length);
// The GPU scanout's half-float conversion kernel as AIR bitcode (GpuScanoutPresenter.h).
const void *nmb_scanout_convert_air(size_t *length);
const void *nmb_function_constant_air(size_t *length);

/* loader: a library exporting vkGetInstanceProcAddr (Turnip on the tablet). cache: a writable directory for translated shaders. */
NmbDevice *nmb_device_open(const char *loader, const char *cache);
void nmb_device_release(NmbDevice *);
size_t nmb_device_name(const NmbDevice *, char *out, size_t size);
uint64_t nmb_device_max_buffer_length(const NmbDevice *);
// Pipeline makings (count, total us, longest us, waited us, failed), sink messages (info, warning+), shader cache hits/misses/writes.
bool nmb_device_work_stats(const NmbDevice *, uint64_t out[10]);
/* The translator source the runtime was built from, which keys the shader cache (build-native-metal-bridge.py). */
const char *nmb_runtime_build(void);
// Linear images made/refused/refused again, shadowed textures, shadow copies, texels copied back, linear passes in place/through copies.
bool nmb_device_texture_counts(const NmbDevice *, uint64_t out[8]);
// No-copy ranges checked, and those whose pages had changed since a buffer was last made over them (reported to the runtime as remapped).
bool nmb_no_copy_remaps(uint64_t out[2]);
/* Artifact lookups and driver pipeline records are separate cache layers. */
struct NmbShaderCacheStats {
    uint64_t hits, misses, rejected, read_failures, writes, write_failures, evictions;
    uint64_t driver_loads, driver_hits, driver_misses, driver_saves, driver_truncated, driver_mismatched, driver_unchanged;
};
bool nmb_device_shader_cache_stats(const NmbDevice *, struct NmbShaderCacheStats *);
size_t nmb_last_warning(char *out, size_t size);
bool nmb_device_max_threads_per_threadgroup(const NmbDevice *, uint64_t out[3]);
/* Device queries the runtime answers by Metal selector: Apple's private counts and flags where the device has an answer of its own,
 * and the public queries. A selector's kind says its result and argument. */
enum {NMB_ANSWER_COUNT = 0, NMB_ANSWER_FLAG = 1, NMB_ANSWER_COUNT_ARG = 2, NMB_ANSWER_FLAG_ARG = 3};
const char *nmb_device_answer_selector(size_t index, uint32_t *kind /* NMB_ANSWER_* */); /* NULL past the last */
bool nmb_device_answer(const NmbDevice *, const char *selector, uint64_t arg, uint64_t *out); /* false: no answer, nmb_last_error says why */
size_t nmb_device_vendor_name(const NmbDevice *, char *out, size_t size);
size_t nmb_device_family_waivers(const NmbDevice *, char *out, size_t size); /* the target families' rows not met, one family per line */
uint64_t nmb_device_linear_bytes_per_row(const NmbDevice *, uint64_t format, uint64_t width); /* 0: refused */

typedef struct NmbBinaryArchive NmbBinaryArchive;
NmbBinaryArchive *nmb_binary_archive_new(NmbDevice *, const char *path /* NULL: empty */);
void nmb_binary_archive_release(NmbBinaryArchive *);
bool nmb_binary_archive_serialize(NmbBinaryArchive *, const char *path);

NmbBuffer *nmb_buffer_new(NmbDevice *, uint64_t length, uint64_t options, const void *bytes /* NULL: zeroed */);
/* Wraps page-aligned memory the caller keeps mapped until the buffer is released (an IOSurface's base, host pages). */
NmbBuffer *nmb_buffer_new_no_copy(NmbDevice *, void *pointer, uint64_t length, uint64_t options);
/* The same with Metal's deallocator: release(context, pointer, length) runs once, on any thread, when the pages' import is freed (the last
 * buffer and every command buffer using them are gone). A refused call (NULL) never runs it. */
NmbBuffer *nmb_buffer_new_no_copy_releasing(NmbDevice *, void *pointer, uint64_t length, uint64_t options,
                                            void (*release)(void *context, void *pointer, uint64_t length), void *context);
void nmb_buffer_release(NmbBuffer *);
void *nmb_buffer_contents(const NmbBuffer *);
uint64_t nmb_buffer_length(const NmbBuffer *);
uint64_t nmb_buffer_gpu_address(const NmbBuffer *);

struct NmbTextureDescriptor {
    uint64_t type, format, width, height, depth, levels, samples, array_length, usage, options;
    uint64_t rotation, framebuffer_only, is_drawable;
};
/* Heaps: type 0 automatic / 1 placement; offset UINT64_MAX asks an automatic heap to place the resource. */
NmbHeap *nmb_heap_new(NmbDevice *, uint64_t type, uint64_t size, uint64_t options);
void nmb_heap_release(NmbHeap *);
uint64_t nmb_heap_size(const NmbHeap *);
uint64_t nmb_heap_used_size(const NmbHeap *);
uint64_t nmb_heap_allocated_size(const NmbHeap *);
uint64_t nmb_heap_max_available_size(const NmbHeap *, uint64_t alignment);
uint64_t nmb_heap_set_purgeable_state(const NmbHeap *, uint64_t state);
NmbBuffer *nmb_heap_new_buffer(const NmbHeap *, uint64_t length, uint64_t options, uint64_t offset);
NmbTexture *nmb_heap_new_texture(const NmbHeap *, const struct NmbTextureDescriptor *, uint64_t offset);
bool nmb_device_heap_texture_size_and_align(const NmbDevice *, const struct NmbTextureDescriptor *, uint64_t out[2]); /* size, align */
bool nmb_device_heap_buffer_size_and_align(const NmbDevice *, uint64_t length, uint64_t options, uint64_t out[2]);
NmbTexture *nmb_texture_new(NmbDevice *, const struct NmbTextureDescriptor *);
/* A linear texture over a buffer's bytes; iosurface marks the buffer as standing for a surface without planes (Metal's IOSurface rules). */
NmbTexture *nmb_texture_new_on_buffer(NmbBuffer *, const struct NmbTextureDescriptor *, uint64_t offset, uint64_t bytes_per_row, bool iosurface);
// A texture over a surface with planes; a two-plane surface gives its OSType, the asked plane, its size and plane 1's offset and rows.
NmbTexture *nmb_texture_new_on_surface(NmbBuffer *, const struct NmbTextureDescriptor *, uint64_t offset, uint64_t bytes_per_row,
                                       uint32_t pixel_format, uint64_t plane, uint64_t width, uint64_t height, uint64_t chroma_offset,
                                       uint64_t chroma_bytes_per_row);
NmbTexture *nmb_texture_new_linear(NmbBuffer *, const struct NmbTextureDescriptor *, uint64_t offset, uint64_t bytes_per_row, uint64_t bytes_per_image);
/* whole: a view of every level and slice as format; else levels and slices are {start, count} and kind the view's texture type. */
NmbTexture *nmb_texture_new_view(NmbTexture *, uint64_t format, uint64_t kind, const uint64_t levels[2], const uint64_t slices[2], bool whole);
/* setPurgeableState: with Metal's raw state; the answer's raw state, 0 where it fails. */
uint64_t nmb_texture_set_purgeable_state(const NmbTexture *, uint64_t state);
uint64_t nmb_buffer_set_purgeable_state(const NmbBuffer *, uint64_t state);
void nmb_texture_release(NmbTexture *);
void nmb_texture_describe(const NmbTexture *, struct NmbTextureDescriptor *out);
bool nmb_texture_replace(NmbTexture *, const uint64_t region[6], uint64_t level, uint64_t slice, const void *bytes, uint64_t bytes_per_row,
                         uint64_t bytes_per_image, uint64_t length);
bool nmb_texture_get_bytes(const NmbTexture *, void *bytes, uint64_t bytes_per_row, uint64_t bytes_per_image, uint64_t length,
                           const uint64_t region[6], uint64_t level, uint64_t slice);

/* A metallib (a universal one gives its AIR64 slice), or with air set bare AIR bitcode. */
NmbLibrary *nmb_library_new(NmbDevice *, const void *bytes, size_t length, bool air);
void nmb_library_release(NmbLibrary *);
uint32_t nmb_library_function_count(const NmbLibrary *);
size_t nmb_library_function_name(const NmbLibrary *, uint32_t index, char *out, size_t size);
NmbFunction *nmb_function_new(NmbLibrary *, const char *name);
/* A function specialized with constant values (newFunctionWithName:constantValues:error:): type is Metal's MTLDataType, name is null for an
 * indexed value; bytes holds the value's bytes. */
struct NmbConstant {uint64_t type, index; const char *name; const void *bytes; uint64_t length;};
NmbFunction *nmb_function_new_specialized(NmbLibrary *, const char *name, const struct NmbConstant *constants, uint32_t count);
// The library's function with every function constant it declares set to zero (shader priming), or null.
NmbFunction *nmb_function_new_zero_constants(NmbLibrary *, const char *name);
/* nmb_function_new_specialized, with every declared constant the caller left out set to zero (false for a bool). */
NmbFunction *nmb_function_new_specialized_zero_rest(NmbLibrary *, const char *name, const struct NmbConstant *constants, uint32_t count);
void nmb_function_release(NmbFunction *);
/* Metal's MTLFunctionType (1 vertex, 2 fragment, 3 kernel, ...). */
uint64_t nmb_function_type(const NmbFunction *);
uint64_t nmb_function_patch_type(const NmbFunction *);              /* MTLPatchType */
int64_t nmb_function_patch_control_point_count(const NmbFunction *); /* -1: no patch, or no count stated */

NmbComputePipeline *nmb_compute_pipeline_new(NmbDevice *, NmbFunction *);
void nmb_compute_pipeline_release(NmbComputePipeline *);
uint64_t nmb_compute_pipeline_max_threads(const NmbComputePipeline *);
uint64_t nmb_compute_pipeline_execution_width(const NmbComputePipeline *);

struct NmbColorAttachment {
    uint64_t format, write_mask, blending, source_rgb, destination_rgb, rgb_operation, source_alpha, destination_alpha, alpha_operation;
};
struct NmbVertexAttribute {uint64_t format, offset, buffer;};
struct NmbVertexLayout {uint64_t stride, step_function, step_rate;};
struct NmbRenderPipelineDescriptor {
    NmbFunction *vertex, *fragment;
    struct NmbColorAttachment colors[8];
    uint64_t depth_format, stencil_format, samples, topology, rasterization;
    uint64_t vertex_inputs; /* 0: no vertex descriptor */
    struct NmbVertexAttribute attributes[31];
    struct NmbVertexLayout layouts[31];
    /* A post-tessellation vertex function's state; tessellates 0 leaves it unset (zeroed descriptors stay untessellated). */
    struct NmbTessellation {
        uint64_t tessellates, max_factor, partition_mode, factor_format, factor_step_function, output_winding, control_point_index_type,
            factor_scale_enabled;
    } tessellation;
};
NmbRenderPipeline *nmb_render_pipeline_new(NmbDevice *, const struct NmbRenderPipelineDescriptor *);
/* Descriptor binaryArchives and MTLPipelineOption bits. NULL options means defaults.
 * The archive array and its handles must remain valid for the duration of this call. */
struct NmbPipelineOptions {uint64_t flags; NmbBinaryArchive *const *archives; size_t archive_count;};
struct NmbComputePipelineDescriptor {
    NmbFunction *function;
    uint64_t max_threads, multiple_of_execution_width, indirect, max_call_stack_depth, adding_binary_functions;
    uint64_t mutability[31];
    /* linkedFunctions.functions and .privateFunctions: visible functions the kernel calls. */
    NmbFunction *const *linked; uint64_t linked_count;
    NmbFunction *const *private_linked; uint64_t private_linked_count;
};
NmbComputePipeline *nmb_compute_pipeline_new_with_options(NmbDevice *, NmbFunction *, const struct NmbPipelineOptions *);
NmbComputePipeline *nmb_compute_pipeline_new_with_descriptor(NmbDevice *, const struct NmbComputePipelineDescriptor *, const struct NmbPipelineOptions *);
NmbRenderPipeline *nmb_render_pipeline_new_with_options(NmbDevice *, const struct NmbRenderPipelineDescriptor *, const struct NmbPipelineOptions *);
/* The archive makes the state as the matching new call does and keeps what it used (addRender/ComputePipelineFunctionsWithDescriptor:). */
bool nmb_binary_archive_add_render(NmbBinaryArchive *, const struct NmbRenderPipelineDescriptor *);
bool nmb_binary_archive_add_compute(NmbBinaryArchive *, NmbFunction *);
bool nmb_binary_archive_add_compute_descriptor(NmbBinaryArchive *, const struct NmbComputePipelineDescriptor *);
void nmb_render_pipeline_release(NmbRenderPipeline *);
// A pipeline's reflection, as its creation's `reflection:` answers it, in JSON: {"vertex":[...],"fragment":[...]} for a render pipeline,
// {"bindings":[...]} for a compute pipeline; each binding has Metal's raw values and a buffer's nested struct/array/pointer types. Null: refused.
typedef struct NmbReflection NmbReflection;
NmbReflection *nmb_render_pipeline_reflection(const struct NmbRenderPipelineDescriptor *);
NmbReflection *nmb_compute_pipeline_reflection(const struct NmbComputePipelineDescriptor *);
const char *nmb_reflection_json(const NmbReflection *);
void nmb_reflection_release(NmbReflection *);

struct NmbSamplerDescriptor {
    uint64_t min_filter, mag_filter, mip_filter, s_address, t_address, r_address, border_color, normalized, max_anisotropy, compare, lod_average;
    float lod_min, lod_max;
};
NmbSampler *nmb_sampler_new(NmbDevice *, const struct NmbSamplerDescriptor *);
void nmb_sampler_release(NmbSampler *);

struct NmbStencil {uint64_t compare, failure, depth_failure, pass, read_mask, write_mask;};
struct NmbDepthStencilDescriptor {uint64_t compare, write; struct NmbStencil front, back;};
NmbDepthStencil *nmb_depth_stencil_new(NmbDevice *, const struct NmbDepthStencilDescriptor *);
void nmb_depth_stencil_release(NmbDepthStencil *);

NmbQueue *nmb_queue_new(NmbDevice *);
void nmb_queue_release(NmbQueue *);
NmbCommandBuffer *nmb_command_buffer_new(NmbQueue *);
void nmb_command_buffer_release(NmbCommandBuffer *);
/* 1 storage dependency, 0 unrelated/already complete, -1 unknown (caller must wait).
 * Includes texture roots, buffer-backed textures, heaps and overlapping imported host ranges.
 * This frontend does not rebind/remap live backing ranges. */
int nmb_command_buffer_uses_buffer(const NmbCommandBuffer *, const NmbBuffer *);
int nmb_command_buffer_uses_texture(const NmbCommandBuffer *, const NmbTexture *);
/* The handler runs once, on a runtime thread, after the command buffer completed or failed. */
bool nmb_command_buffer_on_completed(NmbCommandBuffer *, void (*handler)(void *context), void *context);
/* Runs on actual scheduling, before completion. Not called if submission fails before scheduling.
 * The caller keeps context alive until completion (or a refused commit). */
bool nmb_command_buffer_on_scheduled(NmbCommandBuffer *, void (*handler)(void *context), void *context);
bool nmb_command_buffer_commit(NmbCommandBuffer *);
/* Wait for scheduling and scheduled handlers only; false on a refused wait. */
bool nmb_command_buffer_wait_scheduled(NmbCommandBuffer *);
/* 0 completed in time, 1 timed out (timeout_ms 0 waits without a bound). */
int nmb_command_buffer_wait(NmbCommandBuffer *, uint64_t timeout_ms);
/* Metal's MTLCommandBufferStatus (0 not enqueued .. 4 completed, 5 error). */
uint64_t nmb_command_buffer_status(const NmbCommandBuffer *);
size_t nmb_command_buffer_error(const NmbCommandBuffer *, char *out, size_t size);
// Metal's MTLCommandBufferError code, or 0 for no error / a translation-runtime refusal.
uint64_t nmb_command_buffer_metal_error_code(const NmbCommandBuffer *);
double nmb_command_buffer_gpu_time(const NmbCommandBuffer *, bool end);

/* Events: an MTLEvent (shared false) or an MTLSharedEvent within this process. */
NmbEvent *nmb_event_new(NmbDevice *, bool shared);
void nmb_event_release(NmbEvent *);
uint64_t nmb_shared_event_value(const NmbEvent *);
bool nmb_shared_event_set_value(NmbEvent *, uint64_t value);
/* true once the value is reached within timeout_ms */
bool nmb_shared_event_wait(NmbEvent *, uint64_t value, uint64_t timeout_ms);
/* handler(context, value) runs once on the bridge's listener thread when the value is reached (at once if it already is). */
bool nmb_shared_event_notify(NmbEvent *, uint64_t value, void (*handler)(void *context, uint64_t value), void *context);
bool nmb_command_buffer_signal_event(NmbCommandBuffer *, NmbEvent *, uint64_t value);
bool nmb_command_buffer_wait_event(NmbCommandBuffer *, NmbEvent *, uint64_t value);
NmbFence *nmb_fence_new(NmbDevice *);
void nmb_fence_release(NmbFence *);
/* stages: Metal's MTLRenderStages for a render encoder, ignored otherwise */
bool nmb_encoder_update_fence(NmbEncoder *, NmbFence *, uint64_t stages);
bool nmb_encoder_wait_fence(NmbEncoder *, NmbFence *, uint64_t stages);

NmbEncoder *nmb_compute_encoder_new(NmbCommandBuffer *);
NmbEncoder *nmb_blit_encoder_new(NmbCommandBuffer *);
struct NmbPassAttachment {
    NmbTexture *texture;
    uint64_t level, slice, depth_plane, load, store;
    double clear[4]; /* color, or clear[0] depth / stencil */
    NmbTexture *resolve_texture;
    uint64_t resolve_level, resolve_slice, resolve_depth_plane, resolve_filter;
};
struct NmbRenderPassDescriptor {
    struct NmbPassAttachment colors[8], depth, stencil;
    uint64_t width, height, array_length, samples;
    NmbBuffer *visibility; /* the pass's visibility result buffer, or null */
};
NmbEncoder *nmb_render_encoder_new(NmbCommandBuffer *, const struct NmbRenderPassDescriptor *);
bool nmb_encoder_end(NmbEncoder *);
void nmb_encoder_discard(NmbEncoder *);

/* stage: 0 compute, 1 vertex, 2 fragment */
bool nmb_encoder_set_buffer(NmbEncoder *, uint32_t stage, NmbBuffer *, uint64_t offset, uint32_t index);
bool nmb_encoder_set_buffer_offset(NmbEncoder *, uint32_t stage, uint64_t offset, uint32_t index);
bool nmb_encoder_set_bytes(NmbEncoder *, uint32_t stage, const void *bytes, size_t length, uint32_t index);
bool nmb_encoder_set_texture(NmbEncoder *, uint32_t stage, NmbTexture *, uint32_t index);
bool nmb_encoder_set_sampler(NmbEncoder *, uint32_t stage, NmbSampler *, uint32_t index);

bool nmb_compute_set_pipeline(NmbEncoder *, NmbComputePipeline *);
bool nmb_compute_set_threadgroup_memory(NmbEncoder *, uint32_t length, uint32_t index);
bool nmb_compute_dispatch(NmbEncoder *, const uint64_t size[3], const uint64_t per_group[3], bool threads);
bool nmb_compute_dispatch_indirect(NmbEncoder *, NmbBuffer *, uint64_t offset, const uint64_t per_group[3]);

bool nmb_render_set_pipeline(NmbEncoder *, NmbRenderPipeline *);
bool nmb_render_set_depth_stencil(NmbEncoder *, NmbDepthStencil *);
bool nmb_render_set_viewport(NmbEncoder *, const double viewport[6]);
bool nmb_render_set_scissor(NmbEncoder *, const uint64_t rect[4]);
bool nmb_render_set_raster(NmbEncoder *, uint64_t cull_mode, uint64_t winding, uint64_t fill_mode);
bool nmb_render_set_stencil_reference(NmbEncoder *, uint32_t front, uint32_t back);
bool nmb_render_set_blend_color(NmbEncoder *, const float color[4]);
bool nmb_render_set_depth_bias(NmbEncoder *, float bias, float slope_scale, float clamp);
bool nmb_render_set_line_width(NmbEncoder *, float width);
bool nmb_render_set_visibility_mode(NmbEncoder *, uint64_t mode, uint64_t offset);
bool nmb_render_draw(NmbEncoder *, uint64_t primitive, uint32_t start, uint32_t count, uint32_t instances, uint32_t base_instance);
bool nmb_render_draw_indexed(NmbEncoder *, uint64_t primitive, uint32_t count, uint64_t index_type, NmbBuffer *indices, uint64_t offset,
                             uint32_t instances, int32_t base_vertex, uint32_t base_instance);
/* NULL buffer unsets it. */
bool nmb_render_set_tessellation_factor_buffer(NmbEncoder *, NmbBuffer *, uint64_t offset, uint64_t instance_stride);
bool nmb_render_set_tessellation_factor_scale(NmbEncoder *, float scale);
/* NULL indirect: patch_start, patch_count, instances and base_instance; otherwise read from it. NULL control_point_index: the patches'
 * own control points. NULL patch_index: patches patch_start + i. */
bool nmb_render_draw_patches(NmbEncoder *, uint32_t control_points, uint32_t patch_start, uint32_t patch_count, NmbBuffer *patch_index,
                             uint64_t patch_index_offset, NmbBuffer *control_point_index, uint64_t control_point_index_offset,
                             uint32_t instances, uint32_t base_instance, NmbBuffer *indirect, uint64_t indirect_offset);

bool nmb_blit_copy_buffer(NmbEncoder *, NmbBuffer *source, uint64_t source_offset, NmbBuffer *destination, uint64_t destination_offset,
                          uint64_t size);
bool nmb_blit_fill_buffer(NmbEncoder *, NmbBuffer *, uint64_t start, uint64_t end, uint8_t value);
/* origins and sizes are x,y,z / width,height,depth */
bool nmb_blit_copy_texture(NmbEncoder *, NmbTexture *source, uint64_t source_slice, uint64_t source_level, const uint64_t source_origin[3],
                           const uint64_t size[3], NmbTexture *destination, uint64_t destination_slice, uint64_t destination_level,
                           const uint64_t destination_origin[3]);
bool nmb_blit_buffer_to_texture(NmbEncoder *, NmbBuffer *source, uint64_t offset, uint64_t bytes_per_row, uint64_t bytes_per_image,
                                const uint64_t size[3], NmbTexture *destination, uint64_t slice, uint64_t level, const uint64_t origin[3]);
bool nmb_blit_texture_to_buffer(NmbEncoder *, NmbTexture *source, uint64_t slice, uint64_t level, const uint64_t origin[3],
                                const uint64_t size[3], NmbBuffer *destination, uint64_t offset, uint64_t bytes_per_row,
                                uint64_t bytes_per_image);
bool nmb_blit_generate_mipmaps(NmbEncoder *, NmbTexture *);
#ifdef __cplusplus
}
#endif
#endif
