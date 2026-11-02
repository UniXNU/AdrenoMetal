// SPDX-License-Identifier: MIT
// Turnip smoke test, run by the Adreno Metal plugin from inside the system process that loaded it (under that process's sandbox) once the raw
// userspace test (NativeAdrenoUserTest.h) has passed. It loads the real Mesa Turnip driver (libvulkan_freedreno.dylib, built with the native
// Darwin kernel backend and shipped next to the plugin binary) and drives it through the Vulkan ICD entry points without a loader:
// instance, physical device, device, queue, a host-visible buffer, one vkCmdFillBuffer, a fence, and a read-back of the fill pattern.
// Every stage leaves a progress marker (IOServiceOpen type 0x4147<value>, see NativeAdrenoUserTest.h): 6001 library loaded, 6002 entry points,
// 6003 instance, 6004 physical device, 6005 device, 6007 buffer memory mapped, 6006 command buffer recorded, 6008 submitted, 6009 fence
// signalled, 600a fill pattern verified; then the shader tests: 6100 compute begins, 6101 pipeline, 6102 recorded, 6103 fence, 6104 all words
// right (61fS = compute step S failed); 6200 render begins, 6201 pipeline, 6202 recorded, 6203 fence, 6204 image right (62fS = render step S
// failed); 600b everything destroyed. A setup failure leaves 6fSS (stage SS) followed by 6eRR (RR = -VkResult).
// The outcome is a short text for the kernel's report row:
// "T s<stage> r<VkResult> v<fill words> c<compute words> g<triangle pixels>/<stray pixels> <first Vulkan error message>".
#pragma once
#define VK_NO_PROTOTYPES 1
#include <vulkan/vulkan_core.h>
#include "SharedShaderCache.h"
#include "NativeTurnipShaders.h"   // generated: scripts/gen-turnip-smoke-shaders.py
#include <dlfcn.h>
#include <mach/mach.h>
#include <mach-o/dyld.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

// Vulkan create-info structs are written as {sType} with every other member zero.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wmissing-field-initializers"
namespace turnip {

static char gFirstError[96];

static VKAPI_ATTR VkBool32 VKAPI_CALL onMessage(VkDebugUtilsMessageSeverityFlagBitsEXT severity, VkDebugUtilsMessageTypeFlagsEXT,
                                                const VkDebugUtilsMessengerCallbackDataEXT *data, void *) {
    if ((severity & VK_DEBUG_UTILS_MESSAGE_SEVERITY_ERROR_BIT_EXT) && !gFirstError[0] && data && data->pMessage)
        snprintf(gFirstError, sizeof gFirstError, "%s", data->pMessage);
    return VK_FALSE;
}

struct Result {
    int stage = 0;          // first setup failure (0 = none): 1 library .. 9 fence wait
    int vkResult = 0;
    int verified = -1;      // fill test: words of 1024 that hold the expected value (-1 = not run)
    int computeWords = -1;  // compute test: words of 1024 written by the shader with the expected value (-1 = not run)
    int computeStage = 0;   // first failing step of the compute test (0 = none)
    int renderTriangle = -1, renderOther = -1;  // render test: triangle-colour pixels, pixels that are neither colour (-1 = not run)
    int renderStage = 0;
    bool ok = false;
};

// Device-level entry points the tests use, resolved once through vkGetDeviceProcAddr.
#define TURNIP_DEVICE_FNS(X) \
    X(vkDestroyDevice) X(vkGetDeviceQueue) X(vkCreateCommandPool) X(vkDestroyCommandPool) X(vkAllocateCommandBuffers) \
    X(vkResetCommandBuffer) X(vkBeginCommandBuffer) X(vkEndCommandBuffer) X(vkCmdFillBuffer) X(vkCreateBuffer) X(vkDestroyBuffer) \
    X(vkGetBufferMemoryRequirements) X(vkAllocateMemory) X(vkFreeMemory) X(vkBindBufferMemory) X(vkMapMemory) X(vkUnmapMemory) \
    X(vkCreateFence) X(vkDestroyFence) X(vkQueueSubmit) X(vkWaitForFences) X(vkDeviceWaitIdle) X(vkCreateShaderModule) \
    X(vkDestroyShaderModule) X(vkCreateDescriptorSetLayout) X(vkDestroyDescriptorSetLayout) X(vkCreatePipelineLayout) \
    X(vkDestroyPipelineLayout) X(vkCreateComputePipelines) X(vkCreateGraphicsPipelines) X(vkDestroyPipeline) \
    X(vkCreateDescriptorPool) X(vkDestroyDescriptorPool) X(vkAllocateDescriptorSets) X(vkUpdateDescriptorSets) \
    X(vkCmdBindPipeline) X(vkCmdBindDescriptorSets) X(vkCmdPushConstants) X(vkCmdDispatch) X(vkCmdPipelineBarrier) \
    X(vkCreateImage) X(vkDestroyImage) X(vkGetImageMemoryRequirements) X(vkBindImageMemory) X(vkCreateImageView) \
    X(vkDestroyImageView) X(vkCreateRenderPass) X(vkDestroyRenderPass) X(vkCreateFramebuffer) X(vkDestroyFramebuffer) \
    X(vkCmdBeginRenderPass) X(vkCmdEndRenderPass) X(vkCmdDraw) X(vkCmdCopyImageToBuffer)

struct Device {
    VkDevice device = nullptr;
    VkQueue queue = nullptr;
    VkCommandPool pool = nullptr;
    VkCommandBuffer cmd = nullptr;
    VkPhysicalDeviceMemoryProperties memory = {};
#define TURNIP_DECLARE(name) PFN_##name name = nullptr;
    TURNIP_DEVICE_FNS(TURNIP_DECLARE)
#undef TURNIP_DECLARE

    uint32_t memoryType(uint32_t bits, VkMemoryPropertyFlags flags) const {
        for (uint32_t i = 0; i < memory.memoryTypeCount; ++i)
            if ((bits & (1u << i)) && (memory.memoryTypes[i].propertyFlags & flags) == flags) return i;
        return UINT32_MAX;
    }
    // Records into the one command buffer, submits it with a fresh fence and waits up to five seconds.
    VkResult submitAndWait() {
        VkFenceCreateInfo fci = {VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
        VkFence fence = nullptr;
        VkResult r = vkCreateFence(device, &fci, nullptr, &fence);
        if (r != VK_SUCCESS) return r;
        VkSubmitInfo si = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
        si.commandBufferCount = 1;
        si.pCommandBuffers = &cmd;
        r = vkQueueSubmit(queue, 1, &si, fence);
        if (r == VK_SUCCESS) r = vkWaitForFences(device, 1, &fence, VK_TRUE, 5000000000ull);
        vkDestroyFence(device, fence, nullptr);
        return r;
    }
    VkResult begin() {
        vkResetCommandBuffer(cmd, 0);
        VkCommandBufferBeginInfo bi = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
        bi.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
        return vkBeginCommandBuffer(cmd, &bi);
    }
};

// A host-visible buffer with its memory, mapped for the CPU.
struct HostBuffer {
    VkBuffer buffer = nullptr;
    VkDeviceMemory memory = nullptr;
    volatile uint32_t *words = nullptr;
    VkResult create(Device &d, VkDeviceSize bytes, VkBufferUsageFlags usage) {
        VkBufferCreateInfo bci = {VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO};
        bci.size = bytes;
        bci.usage = usage;
        bci.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
        VkResult r = d.vkCreateBuffer(d.device, &bci, nullptr, &buffer);
        if (r != VK_SUCCESS) return r;
        VkMemoryRequirements req = {};
        d.vkGetBufferMemoryRequirements(d.device, buffer, &req);
        const uint32_t type = d.memoryType(req.memoryTypeBits, VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
        if (type == UINT32_MAX) return VK_ERROR_FEATURE_NOT_PRESENT;
        VkMemoryAllocateInfo mai = {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO};
        mai.allocationSize = req.size;
        mai.memoryTypeIndex = type;
        r = d.vkAllocateMemory(d.device, &mai, nullptr, &memory);
        if (r == VK_SUCCESS) r = d.vkBindBufferMemory(d.device, buffer, memory, 0);
        void *mapped = nullptr;
        if (r == VK_SUCCESS) r = d.vkMapMemory(d.device, memory, 0, bytes, 0, &mapped);
        words = static_cast<volatile uint32_t *>(mapped);
        return r != VK_SUCCESS ? r : (words ? VK_SUCCESS : VK_ERROR_MEMORY_MAP_FAILED);
    }
    void destroy(Device &d) {
        if (words) d.vkUnmapMemory(d.device, memory);
        if (buffer) d.vkDestroyBuffer(d.device, buffer, nullptr);
        if (memory) d.vkFreeMemory(d.device, memory, nullptr);
        *this = HostBuffer();
    }
};

static VkShaderModule shaderModule(Device &d, const uint32_t *words, size_t bytes) {
    VkShaderModuleCreateInfo ci = {VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO};
    ci.codeSize = bytes;
    ci.pCode = words;
    VkShaderModule module = nullptr;
    return d.vkCreateShaderModule(d.device, &ci, nullptr, &module) == VK_SUCCESS ? module : nullptr;
}

static void hostBarrier(Device &d, VkPipelineStageFlags from, VkAccessFlags access) {
    VkMemoryBarrier barrier = {VK_STRUCTURE_TYPE_MEMORY_BARRIER};
    barrier.srcAccessMask = access;
    barrier.dstAccessMask = VK_ACCESS_HOST_READ_BIT;
    d.vkCmdPipelineBarrier(d.cmd, from, VK_PIPELINE_STAGE_HOST_BIT, 0, 1, &barrier, 0, nullptr, 0, nullptr);
}

// Fill: vkCmdFillBuffer of the first half of a 4 KiB buffer (Turnip implements it with its own internal blit). Returns verified words or < 0.
template <typename Marker>
static int fillTest(Device &d, Marker &mark, int &failStage, VkResult &failResult) {
    constexpr VkDeviceSize kBytes = 4096;
    constexpr uint32_t kPattern = 0xc0ffee42u;
    HostBuffer b;
    VkResult r = b.create(d, kBytes, VK_BUFFER_USAGE_TRANSFER_DST_BIT);
    if (r != VK_SUCCESS) {b.destroy(d); failStage = 7; failResult = r; return -1;}
    for (unsigned i = 0; i < kBytes / 4; ++i) b.words[i] = 0xdeadbeef;
    mark(0x6007);
    if ((r = d.begin()) != VK_SUCCESS) {b.destroy(d); failStage = 6; failResult = r; return -1;}
    d.vkCmdFillBuffer(d.cmd, b.buffer, 0, kBytes / 2, kPattern);   // the first half only: the second half must keep 0xdeadbeef
    hostBarrier(d, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_ACCESS_TRANSFER_WRITE_BIT);
    if ((r = d.vkEndCommandBuffer(d.cmd)) != VK_SUCCESS) {b.destroy(d); failStage = 6; failResult = r; return -1;}
    mark(0x6006);
    mark(0x6008);
    if ((r = d.submitAndWait()) != VK_SUCCESS) {b.destroy(d); failStage = 9; failResult = r; return -1;}
    mark(0x6009);
    int good = 0;
    for (unsigned i = 0; i < kBytes / 8; ++i) good += b.words[i] == kPattern;
    for (unsigned i = kBytes / 8; i < kBytes / 4; ++i) good += b.words[i] == 0xdeadbeefu;
    if (good == int(kBytes / 4)) mark(0x600a);
    b.destroy(d);
    return good;
}

// Compute: 16 workgroups of 64 invocations each write v[i] = i * 3 + 0x1000 (push constants) into a storage buffer.
template <typename Marker>
static int computeTest(Device &d, Marker &mark, int &stage) {
    constexpr uint32_t kWords = 1024, kMul = 3, kAdd = 0x1000;
    mark(0x6100);
    HostBuffer b;
    VkShaderModule module = nullptr;
    VkDescriptorSetLayout setLayout = nullptr;
    VkPipelineLayout layout = nullptr;
    VkPipeline pipeline = nullptr;
    VkDescriptorPool pool = nullptr;
    int result = -1;
    auto fail = [&](int s) {stage = s; mark(0x61f0 | s);};
    do {
        if (b.create(d, kWords * 4, VK_BUFFER_USAGE_STORAGE_BUFFER_BIT) != VK_SUCCESS) {fail(1); break;}
        for (uint32_t i = 0; i < kWords; ++i) b.words[i] = 0xdeadbeef;
        if (!(module = shaderModule(d, kSpv_smoke_fill_comp, sizeof kSpv_smoke_fill_comp))) {fail(2); break;}
        VkDescriptorSetLayoutBinding binding = {0, VK_DESCRIPTOR_TYPE_STORAGE_BUFFER, 1, VK_SHADER_STAGE_COMPUTE_BIT, nullptr};
        VkDescriptorSetLayoutCreateInfo dsl = {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO};
        dsl.bindingCount = 1;
        dsl.pBindings = &binding;
        if (d.vkCreateDescriptorSetLayout(d.device, &dsl, nullptr, &setLayout) != VK_SUCCESS) {fail(3); break;}
        VkPushConstantRange push = {VK_SHADER_STAGE_COMPUTE_BIT, 0, 8};
        VkPipelineLayoutCreateInfo pl = {VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
        pl.setLayoutCount = 1;
        pl.pSetLayouts = &setLayout;
        pl.pushConstantRangeCount = 1;
        pl.pPushConstantRanges = &push;
        if (d.vkCreatePipelineLayout(d.device, &pl, nullptr, &layout) != VK_SUCCESS) {fail(4); break;}
        VkComputePipelineCreateInfo cp = {VK_STRUCTURE_TYPE_COMPUTE_PIPELINE_CREATE_INFO};
        cp.stage = {VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO, nullptr, 0, VK_SHADER_STAGE_COMPUTE_BIT, module, "main", nullptr};
        cp.layout = layout;
        if (d.vkCreateComputePipelines(d.device, nullptr, 1, &cp, nullptr, &pipeline) != VK_SUCCESS) {fail(5); break;}
        mark(0x6101);
        VkDescriptorPoolSize size = {VK_DESCRIPTOR_TYPE_STORAGE_BUFFER, 1};
        VkDescriptorPoolCreateInfo dp = {VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO};
        dp.maxSets = 1;
        dp.poolSizeCount = 1;
        dp.pPoolSizes = &size;
        if (d.vkCreateDescriptorPool(d.device, &dp, nullptr, &pool) != VK_SUCCESS) {fail(6); break;}
        VkDescriptorSetAllocateInfo da = {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO};
        da.descriptorPool = pool;
        da.descriptorSetCount = 1;
        da.pSetLayouts = &setLayout;
        VkDescriptorSet set = nullptr;
        if (d.vkAllocateDescriptorSets(d.device, &da, &set) != VK_SUCCESS) {fail(6); break;}
        VkDescriptorBufferInfo info = {b.buffer, 0, kWords * 4};
        VkWriteDescriptorSet write = {VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET};
        write.dstSet = set;
        write.descriptorCount = 1;
        write.descriptorType = VK_DESCRIPTOR_TYPE_STORAGE_BUFFER;
        write.pBufferInfo = &info;
        d.vkUpdateDescriptorSets(d.device, 1, &write, 0, nullptr);
        if (d.begin() != VK_SUCCESS) {fail(7); break;}
        const uint32_t constants[2] = {kMul, kAdd};
        d.vkCmdBindPipeline(d.cmd, VK_PIPELINE_BIND_POINT_COMPUTE, pipeline);
        d.vkCmdBindDescriptorSets(d.cmd, VK_PIPELINE_BIND_POINT_COMPUTE, layout, 0, 1, &set, 0, nullptr);
        d.vkCmdPushConstants(d.cmd, layout, VK_SHADER_STAGE_COMPUTE_BIT, 0, sizeof constants, constants);
        d.vkCmdDispatch(d.cmd, kWords / 64, 1, 1);
        hostBarrier(d, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT, VK_ACCESS_SHADER_WRITE_BIT);
        if (d.vkEndCommandBuffer(d.cmd) != VK_SUCCESS) {fail(7); break;}
        mark(0x6102);
        if (d.submitAndWait() != VK_SUCCESS) {fail(8); break;}
        mark(0x6103);
        int good = 0;
        for (uint32_t i = 0; i < kWords; ++i) good += b.words[i] == i * kMul + kAdd;
        result = good;
        if (good == int(kWords)) mark(0x6104);
    } while (false);
    if (pipeline) d.vkDestroyPipeline(d.device, pipeline, nullptr);
    if (pool) d.vkDestroyDescriptorPool(d.device, pool, nullptr);
    if (layout) d.vkDestroyPipelineLayout(d.device, layout, nullptr);
    if (setLayout) d.vkDestroyDescriptorSetLayout(d.device, setLayout, nullptr);
    if (module) d.vkDestroyShaderModule(d.device, module, nullptr);
    b.destroy(d);
    return result;
}

// Render: a 64x64 RGBA8 colour attachment cleared to blue, one triangle drawn in orange (push constant), copied to a host buffer.
// The triangle covers 512 pixels in exact arithmetic; every other pixel must keep the clear colour.
template <typename Marker>
static void renderTest(Device &d, Marker &mark, int &stage, int &triangle, int &other) {
    constexpr uint32_t kSize = 64;
    mark(0x6200);
    HostBuffer b;
    VkImage image = nullptr;
    VkDeviceMemory imageMemory = nullptr;
    VkImageView view = nullptr;
    VkRenderPass pass = nullptr;
    VkFramebuffer framebuffer = nullptr;
    VkShaderModule vs = nullptr, fs = nullptr;
    VkPipelineLayout layout = nullptr;
    VkPipeline pipeline = nullptr;
    auto fail = [&](int s) {stage = s; mark(0x62f0 | s);};
    do {
        if (b.create(d, kSize * kSize * 4, VK_BUFFER_USAGE_TRANSFER_DST_BIT) != VK_SUCCESS) {fail(1); break;}
        for (uint32_t i = 0; i < kSize * kSize; ++i) b.words[i] = 0x5a5a5a5a;
        VkImageCreateInfo ic = {VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO};
        ic.imageType = VK_IMAGE_TYPE_2D;
        ic.format = VK_FORMAT_R8G8B8A8_UNORM;
        ic.extent = {kSize, kSize, 1};
        ic.mipLevels = 1;
        ic.arrayLayers = 1;
        ic.samples = VK_SAMPLE_COUNT_1_BIT;
        ic.tiling = VK_IMAGE_TILING_OPTIMAL;
        ic.usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT;
        ic.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
        if (d.vkCreateImage(d.device, &ic, nullptr, &image) != VK_SUCCESS) {fail(2); break;}
        VkMemoryRequirements req = {};
        d.vkGetImageMemoryRequirements(d.device, image, &req);
        uint32_t type = d.memoryType(req.memoryTypeBits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT);
        if (type == UINT32_MAX) type = d.memoryType(req.memoryTypeBits, 0);
        VkMemoryAllocateInfo mai = {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO};
        mai.allocationSize = req.size;
        mai.memoryTypeIndex = type;
        if (type == UINT32_MAX || d.vkAllocateMemory(d.device, &mai, nullptr, &imageMemory) != VK_SUCCESS ||
            d.vkBindImageMemory(d.device, image, imageMemory, 0) != VK_SUCCESS) {fail(2); break;}
        VkImageViewCreateInfo vc = {VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
        vc.image = image;
        vc.viewType = VK_IMAGE_VIEW_TYPE_2D;
        vc.format = ic.format;
        vc.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
        if (d.vkCreateImageView(d.device, &vc, nullptr, &view) != VK_SUCCESS) {fail(3); break;}
        VkAttachmentDescription attachment = {0, ic.format, VK_SAMPLE_COUNT_1_BIT, VK_ATTACHMENT_LOAD_OP_CLEAR, VK_ATTACHMENT_STORE_OP_STORE,
                                              VK_ATTACHMENT_LOAD_OP_DONT_CARE, VK_ATTACHMENT_STORE_OP_DONT_CARE, VK_IMAGE_LAYOUT_UNDEFINED,
                                              VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL};
        VkAttachmentReference ref = {0, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};
        VkSubpassDescription subpass = {};
        subpass.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS;
        subpass.colorAttachmentCount = 1;
        subpass.pColorAttachments = &ref;
        VkSubpassDependency after = {0, VK_SUBPASS_EXTERNAL, VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT,
                                     VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT, VK_ACCESS_TRANSFER_READ_BIT, 0};
        VkRenderPassCreateInfo rp = {VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO};
        rp.attachmentCount = 1;
        rp.pAttachments = &attachment;
        rp.subpassCount = 1;
        rp.pSubpasses = &subpass;
        rp.dependencyCount = 1;
        rp.pDependencies = &after;
        if (d.vkCreateRenderPass(d.device, &rp, nullptr, &pass) != VK_SUCCESS) {fail(4); break;}
        VkFramebufferCreateInfo fb = {VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO};
        fb.renderPass = pass;
        fb.attachmentCount = 1;
        fb.pAttachments = &view;
        fb.width = kSize;
        fb.height = kSize;
        fb.layers = 1;
        if (d.vkCreateFramebuffer(d.device, &fb, nullptr, &framebuffer) != VK_SUCCESS) {fail(4); break;}
        vs = shaderModule(d, kSpv_smoke_triangle_vert, sizeof kSpv_smoke_triangle_vert);
        fs = shaderModule(d, kSpv_smoke_triangle_frag, sizeof kSpv_smoke_triangle_frag);
        if (!vs || !fs) {fail(5); break;}
        VkPushConstantRange push = {VK_SHADER_STAGE_FRAGMENT_BIT, 0, 16};
        VkPipelineLayoutCreateInfo pl = {VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
        pl.pushConstantRangeCount = 1;
        pl.pPushConstantRanges = &push;
        if (d.vkCreatePipelineLayout(d.device, &pl, nullptr, &layout) != VK_SUCCESS) {fail(6); break;}
        VkPipelineShaderStageCreateInfo stages[2] = {
            {VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO, nullptr, 0, VK_SHADER_STAGE_VERTEX_BIT, vs, "main", nullptr},
            {VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO, nullptr, 0, VK_SHADER_STAGE_FRAGMENT_BIT, fs, "main", nullptr}};
        VkPipelineVertexInputStateCreateInfo vi = {VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO};
        VkPipelineInputAssemblyStateCreateInfo ia = {VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO};
        ia.topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;
        VkViewport viewport = {0, 0, float(kSize), float(kSize), 0, 1};
        VkRect2D scissor = {{0, 0}, {kSize, kSize}};
        VkPipelineViewportStateCreateInfo vp = {VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO};
        vp.viewportCount = 1;
        vp.pViewports = &viewport;
        vp.scissorCount = 1;
        vp.pScissors = &scissor;
        VkPipelineRasterizationStateCreateInfo rs = {VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO};
        rs.polygonMode = VK_POLYGON_MODE_FILL;
        rs.cullMode = VK_CULL_MODE_NONE;
        rs.frontFace = VK_FRONT_FACE_COUNTER_CLOCKWISE;
        rs.lineWidth = 1.0f;
        VkPipelineMultisampleStateCreateInfo ms = {VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO};
        ms.rasterizationSamples = VK_SAMPLE_COUNT_1_BIT;
        VkPipelineColorBlendAttachmentState blend = {};
        blend.colorWriteMask = 0xf;
        VkPipelineColorBlendStateCreateInfo cb = {VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO};
        cb.attachmentCount = 1;
        cb.pAttachments = &blend;
        VkGraphicsPipelineCreateInfo gp = {VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO};
        gp.stageCount = 2;
        gp.pStages = stages;
        gp.pVertexInputState = &vi;
        gp.pInputAssemblyState = &ia;
        gp.pViewportState = &vp;
        gp.pRasterizationState = &rs;
        gp.pMultisampleState = &ms;
        gp.pColorBlendState = &cb;
        gp.layout = layout;
        gp.renderPass = pass;
        if (d.vkCreateGraphicsPipelines(d.device, nullptr, 1, &gp, nullptr, &pipeline) != VK_SUCCESS) {fail(7); break;}
        mark(0x6201);
        if (d.begin() != VK_SUCCESS) {fail(8); break;}
        VkClearValue clear = {};
        clear.color.float32[2] = 1.0f;
        clear.color.float32[3] = 1.0f;
        VkRenderPassBeginInfo rb = {VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO};
        rb.renderPass = pass;
        rb.framebuffer = framebuffer;
        rb.renderArea = scissor;
        rb.clearValueCount = 1;
        rb.pClearValues = &clear;
        d.vkCmdBeginRenderPass(d.cmd, &rb, VK_SUBPASS_CONTENTS_INLINE);
        d.vkCmdBindPipeline(d.cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipeline);
        const float color[4] = {1.0f, 0.5f, 0.0f, 1.0f};
        d.vkCmdPushConstants(d.cmd, layout, VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof color, color);
        d.vkCmdDraw(d.cmd, 3, 1, 0, 0);
        d.vkCmdEndRenderPass(d.cmd);
        VkBufferImageCopy copy = {};
        copy.imageSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
        copy.imageExtent = {kSize, kSize, 1};
        d.vkCmdCopyImageToBuffer(d.cmd, image, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, b.buffer, 1, &copy);
        hostBarrier(d, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_ACCESS_TRANSFER_WRITE_BIT);
        if (d.vkEndCommandBuffer(d.cmd) != VK_SUCCESS) {fail(8); break;}
        mark(0x6202);
        if (d.submitAndWait() != VK_SUCCESS) {fail(9); break;}
        mark(0x6203);
        // RGBA8 little-endian words: clear = (0, 0, 255, 255), triangle = (255, 127 or 128, 0, 255).
        int tri = 0, rest = 0;
        for (uint32_t i = 0; i < kSize * kSize; ++i) {
            const uint32_t px = b.words[i];
            if (px == 0xffff0000u) continue;
            if (px == 0xff0080ffu || px == 0xff007fffu) ++tri;
            else ++rest;
        }
        triangle = tri;
        other = rest;
        if (rest == 0 && tri >= 480 && tri <= 544) mark(0x6204);
    } while (false);
    if (pipeline) d.vkDestroyPipeline(d.device, pipeline, nullptr);
    if (layout) d.vkDestroyPipelineLayout(d.device, layout, nullptr);
    if (vs) d.vkDestroyShaderModule(d.device, vs, nullptr);
    if (fs) d.vkDestroyShaderModule(d.device, fs, nullptr);
    if (framebuffer) d.vkDestroyFramebuffer(d.device, framebuffer, nullptr);
    if (pass) d.vkDestroyRenderPass(d.device, pass, nullptr);
    if (view) d.vkDestroyImageView(d.device, view, nullptr);
    if (image) d.vkDestroyImage(d.device, image, nullptr);
    if (imageMemory) d.vkFreeMemory(d.device, imageMemory, nullptr);
    b.destroy(d);
}

template <typename Marker>
static Result run(Marker &&mark, const char *libraryPath) {
    Result out;
    auto fail = [&](int stage, int result) {
        out.stage = stage;
        out.vkResult = result;
        mark(0x6f00 | stage);
        mark(0x6e00 | ((-result) & 0xff));
        return out;
    };
    // The Darwin backend reaches the driver the same way the plugin does: the AGX accelerator service with open type 0x4147, ABI methods through
    // an admitted selector. The shader cache uses the same writable per-user directory as the Metal runtime.
    setenv("TU_DARWIN_SERVICE", "IOAcceleratorES", 0);
    const std::string sharedCache = sharedShaderCacheDirectory();
    if (!sharedCache.empty()) configureShaderDiskCache(sharedCache);
    // Concurrent binning (BV running ahead of BR) is a performance feature; its barriers make BR wait for BV timestamps. int-058's
    // first fill hung with the ring stopped inside the IB (no fault), before the kernel framed IBs for both threads. Keep it off
    // until a run with the framing has passed.
    setenv("TU_DEBUG", "nocb", 0);

    void *lib = dlopen(libraryPath, RTLD_NOW | RTLD_LOCAL);
    if (!lib) return fail(1, -1);
    mark(0x6001);
    auto gipa = reinterpret_cast<PFN_vkGetInstanceProcAddr>(dlsym(lib, "vk_icdGetInstanceProcAddr"));
    if (!gipa) return fail(2, -2);
    auto createInstance = reinterpret_cast<PFN_vkCreateInstance>(gipa(nullptr, "vkCreateInstance"));
    if (!createInstance) return fail(2, -3);
    mark(0x6002);

    const char *extensions[] = {VK_EXT_DEBUG_UTILS_EXTENSION_NAME};
    VkDebugUtilsMessengerCreateInfoEXT messenger = {VK_STRUCTURE_TYPE_DEBUG_UTILS_MESSENGER_CREATE_INFO_EXT};
    messenger.messageSeverity = VK_DEBUG_UTILS_MESSAGE_SEVERITY_ERROR_BIT_EXT;
    messenger.messageType = VK_DEBUG_UTILS_MESSAGE_TYPE_GENERAL_BIT_EXT | VK_DEBUG_UTILS_MESSAGE_TYPE_VALIDATION_BIT_EXT;
    messenger.pfnUserCallback = onMessage;
    VkApplicationInfo app = {VK_STRUCTURE_TYPE_APPLICATION_INFO};
    app.pApplicationName = "NativeAdrenoMetal";
    app.apiVersion = VK_API_VERSION_1_1;
    VkInstanceCreateInfo ici = {VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO};
    ici.pNext = &messenger;
    ici.pApplicationInfo = &app;
    ici.enabledExtensionCount = 1;
    ici.ppEnabledExtensionNames = extensions;
    VkInstance instance = nullptr;
    VkResult r = createInstance(&ici, nullptr, &instance);
    if (r != VK_SUCCESS) return fail(3, r);
    mark(0x6003);

#define INSTANCE_FN(name) auto name = reinterpret_cast<PFN_##name>(gipa(instance, #name)); if (!name) return fail(3, -4)
    INSTANCE_FN(vkEnumeratePhysicalDevices);
    INSTANCE_FN(vkGetPhysicalDeviceMemoryProperties);
    INSTANCE_FN(vkGetPhysicalDeviceQueueFamilyProperties);
    INSTANCE_FN(vkCreateDevice);
    INSTANCE_FN(vkGetDeviceProcAddr);
    INSTANCE_FN(vkDestroyInstance);
#undef INSTANCE_FN

    uint32_t count = 1;
    VkPhysicalDevice physical = nullptr;
    r = vkEnumeratePhysicalDevices(instance, &count, &physical);
    if ((r != VK_SUCCESS && r != VK_INCOMPLETE) || !count || !physical) return fail(4, r == VK_SUCCESS ? -5 : r);
    mark(0x6004);

    uint32_t families = 0;
    vkGetPhysicalDeviceQueueFamilyProperties(physical, &families, nullptr);
    if (!families) return fail(4, -6);
    VkQueueFamilyProperties family[8] = {};
    if (families > 8) families = 8;
    vkGetPhysicalDeviceQueueFamilyProperties(physical, &families, family);
    uint32_t queueFamily = families;
    for (uint32_t i = 0; i < families && queueFamily == families; ++i)
        if (family[i].queueFlags & VK_QUEUE_GRAPHICS_BIT) queueFamily = i;
    if (queueFamily == families) return fail(4, -7);

    float priority = 1.0f;
    VkDeviceQueueCreateInfo qci = {VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO};
    qci.queueFamilyIndex = queueFamily;
    qci.queueCount = 1;
    qci.pQueuePriorities = &priority;
    VkDeviceCreateInfo dci = {VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO};
    dci.queueCreateInfoCount = 1;
    dci.pQueueCreateInfos = &qci;
    Device d;
    r = vkCreateDevice(physical, &dci, nullptr, &d.device);
    if (r != VK_SUCCESS) return fail(5, r);
    mark(0x6005);
#define TURNIP_RESOLVE(name) d.name = reinterpret_cast<PFN_##name>(vkGetDeviceProcAddr(d.device, #name)); if (!d.name) return fail(5, -8);
    TURNIP_DEVICE_FNS(TURNIP_RESOLVE)
#undef TURNIP_RESOLVE
    vkGetPhysicalDeviceMemoryProperties(physical, &d.memory);
    d.vkGetDeviceQueue(d.device, queueFamily, 0, &d.queue);
    VkCommandPoolCreateInfo pci = {VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO};
    pci.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
    pci.queueFamilyIndex = queueFamily;
    r = d.vkCreateCommandPool(d.device, &pci, nullptr, &d.pool);
    if (r != VK_SUCCESS) return fail(6, r);
    VkCommandBufferAllocateInfo cai = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO};
    cai.commandPool = d.pool;
    cai.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    cai.commandBufferCount = 1;
    r = d.vkAllocateCommandBuffers(d.device, &cai, &d.cmd);
    if (r != VK_SUCCESS) return fail(6, r);

    // The three tests are independent: a failed or unverified one does not stop the next, unless the GPU stopped retiring work (fence timeout).
    int failStage = 0;
    VkResult failResult = VK_SUCCESS;
    out.verified = fillTest(d, mark, failStage, failResult);
    if (failStage) {
        fail(failStage, failResult);
    }
    if (failStage != 9) {
        out.computeWords = computeTest(d, mark, out.computeStage);
        if (out.computeStage != 8) renderTest(d, mark, out.renderStage, out.renderTriangle, out.renderOther);
    }

    d.vkDeviceWaitIdle(d.device);
    d.vkDestroyCommandPool(d.device, d.pool, nullptr);
    d.vkDestroyDevice(d.device, nullptr);
    vkDestroyInstance(instance, nullptr);
    mark(0x600b);
    out.ok = !failStage && out.verified == 1024 && out.computeWords == 1024 && out.renderOther == 0 && out.renderTriangle >= 480 &&
             out.renderTriangle <= 544;
    return out;
}

// Watchdog. run() executes on a worker thread; if it has not returned after 15, 45 and 90 seconds, the calling thread samples the worker and
// sends "W <last marker> s<run state> u<user us>/k<system us> over 300 ms, then the stack": the PC, LR and frame-pointer return addresses, each
// as <image letter><offset in that image> (v = libvulkan_freedreno.dylib; offsets resolve against the unstripped build). A worker that is
// running in the kernel (system time only, or no time at all: a core parked by the panic screen) is not suspended, because thread_suspend
// would wait for it forever; its line ends in "kern".
struct Watched {
    const char *libraryPath;
    void (*mark)(void *, uint32_t);
    void *markContext;
    volatile uint32_t lastMark;
    volatile int done;
    Result result;
};

static void *watchedWorker(void *argument) {
    Watched *w = static_cast<Watched *>(argument);
    w->result = run([w](uint32_t value) { w->lastMark = value; w->mark(w->markContext, value); }, w->libraryPath);
    __atomic_store_n(&w->done, 1, __ATOMIC_RELEASE);
    return nullptr;
}

static size_t frameText(char *out, size_t size, uint64_t address) {
    Dl_info info = {};
    if (!address || !dladdr(reinterpret_cast<const void *>(address), &info) || !info.dli_fbase)
        return (size_t)snprintf(out, size, " ?%llx", (unsigned long long)address);
    const char *name = info.dli_fname ? strrchr(info.dli_fname, '/') : nullptr;
    name = name ? name + 1 : (info.dli_fname ? info.dli_fname : "?");
    const char tag = !strncmp(name, "libvulkan", 9) ? 'v' : name[0];
    return (size_t)snprintf(out, size, " %c%llx", tag, (unsigned long long)(address - reinterpret_cast<uint64_t>(info.dli_fbase)));
}

static void sample(Watched &w, pthread_t worker, char *text, size_t size) {
    const mach_port_t thread = pthread_mach_thread_np(worker);
    thread_basic_info_data_t a = {}, b = {};
    mach_msg_type_number_t count = THREAD_BASIC_INFO_COUNT;
    thread_info(thread, THREAD_BASIC_INFO, reinterpret_cast<thread_info_t>(&a), &count);
    usleep(300000);
    count = THREAD_BASIC_INFO_COUNT;
    thread_info(thread, THREAD_BASIC_INFO, reinterpret_cast<thread_info_t>(&b), &count);
    auto us = [](time_value_t t) { return int64_t(t.seconds) * 1000000 + t.microseconds; };
    const int64_t du = us(b.user_time) - us(a.user_time), dk = us(b.system_time) - us(a.system_time);
    size_t at = (size_t)snprintf(text, size, "W %x s%d u%lld/k%lld", unsigned(w.lastMark), b.run_state, (long long)du, (long long)dk);
    if (b.run_state == TH_STATE_RUNNING && du <= 0) {
        snprintf(text + at, size - at, " kern");
        return;
    }
    if (thread_suspend(thread) != KERN_SUCCESS) {snprintf(text + at, size - at, " nosusp"); return;}
    arm_thread_state64_t state = {};
    count = ARM_THREAD_STATE64_COUNT;
    if (thread_get_state(thread, ARM_THREAD_STATE64, reinterpret_cast<thread_state_t>(&state), &count) == KERN_SUCCESS) {
        const uint64_t pc = arm_thread_state64_get_pc(state), lr = arm_thread_state64_get_lr(state);
        uint64_t fp = arm_thread_state64_get_fp(state);
        at += frameText(text + at, size - at, pc);
        if (at < size) at += frameText(text + at, size - at, lr);
        const uint64_t low = reinterpret_cast<uint64_t>(pthread_get_stackaddr_np(worker)) - pthread_get_stacksize_np(worker);
        const uint64_t high = reinterpret_cast<uint64_t>(pthread_get_stackaddr_np(worker));
        for (unsigned i = 0; i < 8 && at + 4 < size && fp >= low && fp + 16 <= high && !(fp & 7); ++i) {
            const uint64_t next = reinterpret_cast<const uint64_t *>(fp)[0], ret = reinterpret_cast<const uint64_t *>(fp)[1];
            if (ret != lr || i) at += frameText(text + at, size - at, ret);
            if (next <= fp) break;
            fp = next;
        }
    }
    thread_resume(thread);
}

template <typename Marker, typename Reporter>
static Result runWatched(Marker &mark, Reporter &&report, const char *libraryPath) {
    Watched *w = new Watched{};
    w->libraryPath = libraryPath;
    w->markContext = &mark;
    w->mark = [](void *context, uint32_t value) { (*static_cast<Marker *>(context))(value); };
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setstacksize(&attr, 8u << 20);   // Mesa's NIR/ir3 passes recurse; the default 512 KiB secondary stack is small
    pthread_t worker;
    const bool started = pthread_create(&worker, &attr, watchedWorker, w) == 0;
    pthread_attr_destroy(&attr);
    if (!started) {Result r; r.stage = 0; r.vkResult = -12; return r;}
    static const unsigned checkpoints[] = {15, 45, 90};
    unsigned next = 0;
    for (unsigned tenths = 0; !__atomic_load_n(&w->done, __ATOMIC_ACQUIRE); ++tenths) {
        usleep(100000);
        if (next < 3 && tenths == checkpoints[next] * 10) {
            char text[64] = {};
            sample(*w, worker, text, sizeof text);
            report(text);
            ++next;
        }
    }
    pthread_join(worker, nullptr);
    Result result = w->result;
    delete w;
    return result;
}

// Path of libvulkan_freedreno.dylib: next to the image this code is part of (the plugin binary inside the bundle).
static bool libraryPath(char *path, size_t size) {
    Dl_info info = {};
    if (!dladdr(reinterpret_cast<const void *>(&libraryPath), &info) || !info.dli_fname) return false;
    snprintf(path, size, "%s", info.dli_fname);
    char *slash = strrchr(path, '/');
    if (!slash) return false;
    snprintf(slash + 1, size - size_t(slash + 1 - path), "libvulkan_freedreno.dylib");
    return true;
}

}  // namespace turnip
#pragma clang diagnostic pop
