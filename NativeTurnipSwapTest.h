// SPDX-License-Identifier: MIT
// GMEM colour order test (boot-arg sheng-swaptest=1, once, in the render server): which byte order each path between tile memory and
// memory leaves in a linear BGRA8 image, rendered in system memory and in GMEM. The image is 3048 x 64, as wide as the render server's
// surface (3048 is not a whole number of GMEM blocks, so an imported image takes the unpadded load and store paths). Two kinds: device memory,
// and host memory imported (as an IOSurface's pages are). Its memory starts as B=10 G=20 R=30 A=255 in every pixel; one render pass that loads
// and stores it then does, by region (the scissor):
//   A  x    0..1015  a draw of R=0.8 G=0.4 B=0.2 (stored bytes should read 33 66 cc as B G R)
//   B  x 1016..2031  nothing (the load and the store: 0a 14 1e)
//   C  x 2032..3047, rows  0..31  the input attachment's red written as grey (the loaded red, 30: 1e 1e 1e)
//   D  x 2032..3047, rows 32..63  the draw of A, then the input attachment's red as grey (the drawn red, 204: cc cc cc)
// Each kind and mode sends one line through the GPU trace (event 65): "swap <kind><mode> A<bgr> B<bgr> C<bgr> D<bgr> m<passes> e<err>".
// The image is filled and read back by GPU copies through a host-coherent buffer: imported pages are mapped non-coherent for the GPU, so CPU
// writes and reads of them saw stale cache lines (swaptest2: the system memory pass's draw missing from host memory).
// The mode is set for this thread only (tu_sheng_set_thread_render_mode: 's' system memory, 'g' GMEM where the pass can).
#pragma once
#include "NativeTurnipSmoke.h"
#include "NativeTurnipShaders.h"
#include <stdlib.h>

#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wmissing-field-initializers"
namespace turnip {
namespace swaptest {

constexpr uint32_t kWidth = 3048, kHeight = 64;

struct Fns {
    PFN_vkGetImageSubresourceLayout vkGetImageSubresourceLayout = nullptr;
    PFN_vkCmdSetScissor vkCmdSetScissor = nullptr;
    PFN_vkCmdSetViewport vkCmdSetViewport = nullptr;
    PFN_vkGetMemoryHostPointerPropertiesEXT vkGetMemoryHostPointerPropertiesEXT = nullptr;
    PFN_vkCmdCopyBufferToImage vkCmdCopyBufferToImage = nullptr;
};

struct Target {
    VkImage image = nullptr;
    VkDeviceMemory memory = nullptr;
    VkImageView view = nullptr;
    uint8_t *bytes = nullptr;   // the image's memory as the CPU sees it
    void *host = nullptr;       // the imported allocation (host kind)
    VkDeviceSize pitch = 0;
};

static void destroy(Device &d, Target &t) {
    if (t.view) d.vkDestroyImageView(d.device, t.view, nullptr);
    if (t.image) d.vkDestroyImage(d.device, t.image, nullptr);
    if (t.memory) {
        if (!t.host) d.vkUnmapMemory(d.device, t.memory);
        d.vkFreeMemory(d.device, t.memory, nullptr);
    }
    free(t.host);
    t = Target();
}

// 0 or the failing step.
static int makeTarget(Device &d, Fns &f, bool hostKind, Target &t) {
    VkExternalMemoryImageCreateInfo external = {VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO};
    external.handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_HOST_ALLOCATION_BIT_EXT;
    VkImageCreateInfo ic = {VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO};
    ic.pNext = hostKind ? &external : nullptr;
    ic.imageType = VK_IMAGE_TYPE_2D;
    ic.format = VK_FORMAT_B8G8R8A8_UNORM;
    ic.extent = {kWidth, kHeight, 1};
    ic.mipLevels = 1;
    ic.arrayLayers = 1;
    ic.samples = VK_SAMPLE_COUNT_1_BIT;
    ic.tiling = VK_IMAGE_TILING_LINEAR;
    ic.usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_INPUT_ATTACHMENT_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT;
    ic.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    if (d.vkCreateImage(d.device, &ic, nullptr, &t.image) != VK_SUCCESS) return 1;
    VkImageSubresource sub = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0};
    VkSubresourceLayout layout = {};
    f.vkGetImageSubresourceLayout(d.device, t.image, &sub, &layout);
    t.pitch = layout.rowPitch;
    VkMemoryRequirements req = {};
    d.vkGetImageMemoryRequirements(d.device, t.image, &req);
    VkMemoryAllocateInfo mai = {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO};
    VkImportMemoryHostPointerInfoEXT import = {VK_STRUCTURE_TYPE_IMPORT_MEMORY_HOST_POINTER_INFO_EXT};
    if (hostKind) {
        if (!f.vkGetMemoryHostPointerPropertiesEXT) return 2;
        const size_t size = (req.size + 16383) & ~size_t(16383);
        if (posix_memalign(&t.host, 16384, size) || !t.host) return 2;
        VkMemoryHostPointerPropertiesEXT props = {VK_STRUCTURE_TYPE_MEMORY_HOST_POINTER_PROPERTIES_EXT};
        if (f.vkGetMemoryHostPointerPropertiesEXT(d.device, VK_EXTERNAL_MEMORY_HANDLE_TYPE_HOST_ALLOCATION_BIT_EXT, t.host, &props) !=
            VK_SUCCESS) return 3;
        import.handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_HOST_ALLOCATION_BIT_EXT;
        import.pHostPointer = t.host;
        mai.pNext = &import;
        mai.allocationSize = size;
        mai.memoryTypeIndex = d.memoryType(req.memoryTypeBits & props.memoryTypeBits, 0);
        t.bytes = static_cast<uint8_t *>(t.host);
    } else {
        mai.allocationSize = req.size;
        mai.memoryTypeIndex = d.memoryType(req.memoryTypeBits, VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
    }
    if (mai.memoryTypeIndex == UINT32_MAX) return 4;
    if (d.vkAllocateMemory(d.device, &mai, nullptr, &t.memory) != VK_SUCCESS) return 5;
    if (d.vkBindImageMemory(d.device, t.image, t.memory, 0) != VK_SUCCESS) return 6;
    if (!hostKind) {
        void *mapped = nullptr;
        if (d.vkMapMemory(d.device, t.memory, 0, VK_WHOLE_SIZE, 0, &mapped) != VK_SUCCESS || !mapped) return 7;
        t.bytes = static_cast<uint8_t *>(mapped) + layout.offset;
    } else {
        t.bytes += layout.offset;
    }
    VkImageViewCreateInfo vc = {VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO};
    vc.image = t.image;
    vc.viewType = VK_IMAGE_VIEW_TYPE_2D;
    vc.format = ic.format;
    vc.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    return d.vkCreateImageView(d.device, &vc, nullptr, &t.view) == VK_SUCCESS ? 0 : 8;
}

// One pass over target t in the thread's render mode; 0 or the failing step (20 and up).
static int renderOnce(Device &d, Fns &f, Target &t, HostBuffer &upload, HostBuffer &staging) {
    for (uint32_t i = 0; i < kWidth * kHeight; ++i) upload.words[i] = 0xff1e140au;   // B=0a G=14 R=1e A=ff
    for (uint32_t i = 0; i < kWidth * kHeight; ++i) staging.words[i] = 0;
    VkRenderPass pass = nullptr;
    VkFramebuffer framebuffer = nullptr;
    VkShaderModule vs = nullptr, fsConst = nullptr, fsFetch = nullptr;
    VkDescriptorSetLayout setLayout = nullptr;
    VkPipelineLayout layout = nullptr;
    VkPipeline drawPipe = nullptr, fetchPipe = nullptr;
    VkDescriptorPool pool = nullptr;
    int failed = 0;
    do {
        VkAttachmentDescription attachment = {0, VK_FORMAT_B8G8R8A8_UNORM, VK_SAMPLE_COUNT_1_BIT, VK_ATTACHMENT_LOAD_OP_LOAD,
                                              VK_ATTACHMENT_STORE_OP_STORE, VK_ATTACHMENT_LOAD_OP_DONT_CARE, VK_ATTACHMENT_STORE_OP_DONT_CARE,
                                              VK_IMAGE_LAYOUT_GENERAL, VK_IMAGE_LAYOUT_GENERAL};
        VkAttachmentReference color = {0, VK_IMAGE_LAYOUT_GENERAL}, input = {0, VK_IMAGE_LAYOUT_GENERAL};
        VkSubpassDescription subpass = {};
        subpass.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS;
        subpass.inputAttachmentCount = 1;
        subpass.pInputAttachments = &input;
        subpass.colorAttachmentCount = 1;
        subpass.pColorAttachments = &color;
        VkSubpassDependency deps[2] = {
            {0, 0, VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT, VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT, VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT,
             VK_ACCESS_INPUT_ATTACHMENT_READ_BIT, VK_DEPENDENCY_BY_REGION_BIT},
            {0, VK_SUBPASS_EXTERNAL, VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT, VK_PIPELINE_STAGE_HOST_BIT,
             VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT, VK_ACCESS_HOST_READ_BIT, 0}};
        VkRenderPassCreateInfo rp = {VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO};
        rp.attachmentCount = 1;
        rp.pAttachments = &attachment;
        rp.subpassCount = 1;
        rp.pSubpasses = &subpass;
        rp.dependencyCount = 2;
        rp.pDependencies = deps;
        if (d.vkCreateRenderPass(d.device, &rp, nullptr, &pass) != VK_SUCCESS) {failed = 20; break;}
        VkFramebufferCreateInfo fb = {VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO};
        fb.renderPass = pass;
        fb.attachmentCount = 1;
        fb.pAttachments = &t.view;
        fb.width = kWidth;
        fb.height = kHeight;
        fb.layers = 1;
        if (d.vkCreateFramebuffer(d.device, &fb, nullptr, &framebuffer) != VK_SUCCESS) {failed = 21; break;}
        vs = shaderModule(d, kSpv_smoke_swap_vert, sizeof kSpv_smoke_swap_vert);
        fsConst = shaderModule(d, kSpv_smoke_triangle_frag, sizeof kSpv_smoke_triangle_frag);
        fsFetch = shaderModule(d, kSpv_smoke_swap_fetch_frag, sizeof kSpv_smoke_swap_fetch_frag);
        if (!vs || !fsConst || !fsFetch) {failed = 22; break;}
        VkDescriptorSetLayoutBinding binding = {0, VK_DESCRIPTOR_TYPE_INPUT_ATTACHMENT, 1, VK_SHADER_STAGE_FRAGMENT_BIT, nullptr};
        VkDescriptorSetLayoutCreateInfo sl = {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO};
        sl.bindingCount = 1;
        sl.pBindings = &binding;
        if (d.vkCreateDescriptorSetLayout(d.device, &sl, nullptr, &setLayout) != VK_SUCCESS) {failed = 23; break;}
        VkPushConstantRange push = {VK_SHADER_STAGE_FRAGMENT_BIT, 0, 16};
        VkPipelineLayoutCreateInfo pl = {VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
        pl.setLayoutCount = 1;
        pl.pSetLayouts = &setLayout;
        pl.pushConstantRangeCount = 1;
        pl.pPushConstantRanges = &push;
        if (d.vkCreatePipelineLayout(d.device, &pl, nullptr, &layout) != VK_SUCCESS) {failed = 24; break;}
        VkDescriptorPoolSize size = {VK_DESCRIPTOR_TYPE_INPUT_ATTACHMENT, 1};
        VkDescriptorPoolCreateInfo dp = {VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO};
        dp.maxSets = 1;
        dp.poolSizeCount = 1;
        dp.pPoolSizes = &size;
        if (d.vkCreateDescriptorPool(d.device, &dp, nullptr, &pool) != VK_SUCCESS) {failed = 25; break;}
        VkDescriptorSetAllocateInfo da = {VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO};
        da.descriptorPool = pool;
        da.descriptorSetCount = 1;
        da.pSetLayouts = &setLayout;
        VkDescriptorSet set = nullptr;
        if (d.vkAllocateDescriptorSets(d.device, &da, &set) != VK_SUCCESS) {failed = 25; break;}
        VkDescriptorImageInfo image = {nullptr, t.view, VK_IMAGE_LAYOUT_GENERAL};
        VkWriteDescriptorSet write = {VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET};
        write.dstSet = set;
        write.descriptorCount = 1;
        write.descriptorType = VK_DESCRIPTOR_TYPE_INPUT_ATTACHMENT;
        write.pImageInfo = &image;
        d.vkUpdateDescriptorSets(d.device, 1, &write, 0, nullptr);

        VkPipelineVertexInputStateCreateInfo vi = {VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO};
        VkPipelineInputAssemblyStateCreateInfo ia = {VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO};
        ia.topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;
        VkPipelineViewportStateCreateInfo vp = {VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO};
        vp.viewportCount = 1;
        vp.scissorCount = 1;
        VkPipelineRasterizationStateCreateInfo rs = {VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO};
        rs.polygonMode = VK_POLYGON_MODE_FILL;
        rs.cullMode = VK_CULL_MODE_NONE;
        rs.lineWidth = 1.0f;
        VkPipelineMultisampleStateCreateInfo ms = {VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO};
        ms.rasterizationSamples = VK_SAMPLE_COUNT_1_BIT;
        VkPipelineColorBlendAttachmentState blend = {};
        blend.colorWriteMask = 0xf;
        VkPipelineColorBlendStateCreateInfo cb = {VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO};
        cb.attachmentCount = 1;
        cb.pAttachments = &blend;
        const VkDynamicState dynamic[2] = {VK_DYNAMIC_STATE_VIEWPORT, VK_DYNAMIC_STATE_SCISSOR};
        VkPipelineDynamicStateCreateInfo dyn = {VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO};
        dyn.dynamicStateCount = 2;
        dyn.pDynamicStates = dynamic;
        VkPipelineShaderStageCreateInfo stages[2] = {
            {VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO, nullptr, 0, VK_SHADER_STAGE_VERTEX_BIT, vs, "main", nullptr},
            {VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO, nullptr, 0, VK_SHADER_STAGE_FRAGMENT_BIT, fsConst, "main", nullptr}};
        VkGraphicsPipelineCreateInfo gp = {VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO};
        gp.stageCount = 2;
        gp.pStages = stages;
        gp.pVertexInputState = &vi;
        gp.pInputAssemblyState = &ia;
        gp.pViewportState = &vp;
        gp.pRasterizationState = &rs;
        gp.pMultisampleState = &ms;
        gp.pColorBlendState = &cb;
        gp.pDynamicState = &dyn;
        gp.layout = layout;
        gp.renderPass = pass;
        if (d.vkCreateGraphicsPipelines(d.device, nullptr, 1, &gp, nullptr, &drawPipe) != VK_SUCCESS) {failed = 26; break;}
        stages[1].module = fsFetch;
        if (d.vkCreateGraphicsPipelines(d.device, nullptr, 1, &gp, nullptr, &fetchPipe) != VK_SUCCESS) {failed = 27; break;}

        if (d.begin() != VK_SUCCESS) {failed = 28; break;}
        VkBufferImageCopy copy = {};
        copy.imageSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
        copy.imageExtent = {kWidth, kHeight, 1};
        f.vkCmdCopyBufferToImage(d.cmd, upload.buffer, t.image, VK_IMAGE_LAYOUT_GENERAL, 1, &copy);
        VkMemoryBarrier filled = {VK_STRUCTURE_TYPE_MEMORY_BARRIER, nullptr, VK_ACCESS_TRANSFER_WRITE_BIT,
                                  VK_ACCESS_COLOR_ATTACHMENT_READ_BIT | VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT};
        d.vkCmdPipelineBarrier(d.cmd, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT, 0, 1, &filled, 0, nullptr,
                               0, nullptr);
        VkRenderPassBeginInfo rb = {VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO};
        rb.renderPass = pass;
        rb.framebuffer = framebuffer;
        rb.renderArea = {{0, 0}, {kWidth, kHeight}};
        d.vkCmdBeginRenderPass(d.cmd, &rb, VK_SUBPASS_CONTENTS_INLINE);
        const VkViewport viewport = {0, 0, float(kWidth), float(kHeight), 0, 1};
        f.vkCmdSetViewport(d.cmd, 0, 1, &viewport);
        d.vkCmdBindPipeline(d.cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, drawPipe);
        const float drawn[4] = {0.8f, 0.4f, 0.2f, 1.0f};
        d.vkCmdPushConstants(d.cmd, layout, VK_SHADER_STAGE_FRAGMENT_BIT, 0, sizeof drawn, drawn);
        const VkRect2D a = {{0, 0}, {1016, kHeight}}, dRegion = {{2032, 32}, {1016, 32}}, cd = {{2032, 0}, {1016, kHeight}};
        f.vkCmdSetScissor(d.cmd, 0, 1, &a);
        d.vkCmdDraw(d.cmd, 3, 1, 0, 0);
        f.vkCmdSetScissor(d.cmd, 0, 1, &dRegion);
        d.vkCmdDraw(d.cmd, 3, 1, 0, 0);
        VkMemoryBarrier barrier = {VK_STRUCTURE_TYPE_MEMORY_BARRIER, nullptr, VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT,
                                   VK_ACCESS_INPUT_ATTACHMENT_READ_BIT};
        d.vkCmdPipelineBarrier(d.cmd, VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT, VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT,
                               VK_DEPENDENCY_BY_REGION_BIT, 1, &barrier, 0, nullptr, 0, nullptr);
        d.vkCmdBindPipeline(d.cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, fetchPipe);
        d.vkCmdBindDescriptorSets(d.cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, layout, 0, 1, &set, 0, nullptr);
        f.vkCmdSetScissor(d.cmd, 0, 1, &cd);
        d.vkCmdDraw(d.cmd, 3, 1, 0, 0);
        d.vkCmdEndRenderPass(d.cmd);
        VkMemoryBarrier stored = {VK_STRUCTURE_TYPE_MEMORY_BARRIER, nullptr, VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT, VK_ACCESS_TRANSFER_READ_BIT};
        d.vkCmdPipelineBarrier(d.cmd, VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 1, &stored, 0, nullptr,
                               0, nullptr);
        d.vkCmdCopyImageToBuffer(d.cmd, t.image, VK_IMAGE_LAYOUT_GENERAL, staging.buffer, 1, &copy);
        hostBarrier(d, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_ACCESS_TRANSFER_WRITE_BIT);
        if (d.vkEndCommandBuffer(d.cmd) != VK_SUCCESS) {failed = 28; break;}
        if (d.submitAndWait() != VK_SUCCESS) {failed = 29; break;}
    } while (false);
    if (fetchPipe) d.vkDestroyPipeline(d.device, fetchPipe, nullptr);
    if (drawPipe) d.vkDestroyPipeline(d.device, drawPipe, nullptr);
    if (pool) d.vkDestroyDescriptorPool(d.device, pool, nullptr);
    if (layout) d.vkDestroyPipelineLayout(d.device, layout, nullptr);
    if (setLayout) d.vkDestroyDescriptorSetLayout(d.device, setLayout, nullptr);
    if (vs) d.vkDestroyShaderModule(d.device, vs, nullptr);
    if (fsConst) d.vkDestroyShaderModule(d.device, fsConst, nullptr);
    if (fsFetch) d.vkDestroyShaderModule(d.device, fsFetch, nullptr);
    if (framebuffer) d.vkDestroyFramebuffer(d.device, framebuffer, nullptr);
    if (pass) d.vkDestroyRenderPass(d.device, pass, nullptr);
    return failed;
}

// Runs every kind and mode and hands each result line to report.
template <typename Report>
static void run(const char *libraryPath, Report &&report) {
    char line[96];
    void *lib = dlopen(libraryPath, RTLD_NOW | RTLD_LOCAL);
    auto setMode = lib ? reinterpret_cast<void (*)(char)>(dlsym(lib, "tu_sheng_set_thread_render_mode")) : nullptr;
    auto gipa = lib ? reinterpret_cast<PFN_vkGetInstanceProcAddr>(dlsym(lib, "vk_icdGetInstanceProcAddr")) : nullptr;
    if (!setMode || !gipa) {report("swap setup e1"); return;}
    auto createInstance = reinterpret_cast<PFN_vkCreateInstance>(gipa(nullptr, "vkCreateInstance"));
    VkApplicationInfo app = {VK_STRUCTURE_TYPE_APPLICATION_INFO};
    app.pApplicationName = "NativeAdrenoSwapTest";
    app.apiVersion = VK_API_VERSION_1_1;
    VkInstanceCreateInfo ici = {VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO};
    ici.pApplicationInfo = &app;
    VkInstance instance = nullptr;
    if (!createInstance || createInstance(&ici, nullptr, &instance) != VK_SUCCESS) {report("swap setup e2"); return;}
    auto enumerate = reinterpret_cast<PFN_vkEnumeratePhysicalDevices>(gipa(instance, "vkEnumeratePhysicalDevices"));
    auto memoryProps = reinterpret_cast<PFN_vkGetPhysicalDeviceMemoryProperties>(gipa(instance, "vkGetPhysicalDeviceMemoryProperties"));
    auto extensions = reinterpret_cast<PFN_vkEnumerateDeviceExtensionProperties>(gipa(instance, "vkEnumerateDeviceExtensionProperties"));
    auto createDevice = reinterpret_cast<PFN_vkCreateDevice>(gipa(instance, "vkCreateDevice"));
    auto deviceProc = reinterpret_cast<PFN_vkGetDeviceProcAddr>(gipa(instance, "vkGetDeviceProcAddr"));
    auto destroyInstance = reinterpret_cast<PFN_vkDestroyInstance>(gipa(instance, "vkDestroyInstance"));
    uint32_t count = 1;
    VkPhysicalDevice physical = nullptr;
    if (!enumerate || !memoryProps || !extensions || !createDevice || !deviceProc ||
        enumerate(instance, &count, &physical) < 0 || !physical) {report("swap setup e3"); return;}
    bool hostImport = false;
    uint32_t extensionCount = 0;
    extensions(physical, nullptr, &extensionCount, nullptr);
    VkExtensionProperties ext[256];
    if (extensionCount > 256) extensionCount = 256;
    extensions(physical, nullptr, &extensionCount, ext);
    for (uint32_t i = 0; i < extensionCount; ++i)
        hostImport |= !strcmp(ext[i].extensionName, VK_EXT_EXTERNAL_MEMORY_HOST_EXTENSION_NAME);
    float priority = 1.0f;
    VkDeviceQueueCreateInfo qci = {VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO};
    qci.queueCount = 1;
    qci.pQueuePriorities = &priority;
    const char *wanted[] = {VK_EXT_EXTERNAL_MEMORY_HOST_EXTENSION_NAME};
    VkDeviceCreateInfo dci = {VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO};
    dci.queueCreateInfoCount = 1;
    dci.pQueueCreateInfos = &qci;
    dci.enabledExtensionCount = hostImport ? 1 : 0;
    dci.ppEnabledExtensionNames = wanted;
    Device d;
    if (createDevice(physical, &dci, nullptr, &d.device) != VK_SUCCESS) {report("swap setup e4"); destroyInstance(instance, nullptr); return;}
    bool resolved = true;
#define SWAP_RESOLVE(name) d.name = reinterpret_cast<PFN_##name>(deviceProc(d.device, #name)); resolved &= d.name != nullptr;
    TURNIP_DEVICE_FNS(SWAP_RESOLVE)
#undef SWAP_RESOLVE
    Fns f;
    f.vkGetImageSubresourceLayout = reinterpret_cast<PFN_vkGetImageSubresourceLayout>(deviceProc(d.device, "vkGetImageSubresourceLayout"));
    f.vkCmdSetScissor = reinterpret_cast<PFN_vkCmdSetScissor>(deviceProc(d.device, "vkCmdSetScissor"));
    f.vkCmdSetViewport = reinterpret_cast<PFN_vkCmdSetViewport>(deviceProc(d.device, "vkCmdSetViewport"));
    f.vkCmdCopyBufferToImage = reinterpret_cast<PFN_vkCmdCopyBufferToImage>(deviceProc(d.device, "vkCmdCopyBufferToImage"));
    if (hostImport)
        f.vkGetMemoryHostPointerPropertiesEXT =
            reinterpret_cast<PFN_vkGetMemoryHostPointerPropertiesEXT>(deviceProc(d.device, "vkGetMemoryHostPointerPropertiesEXT"));
    memoryProps(physical, &d.memory);
    d.vkGetDeviceQueue(d.device, 0, 0, &d.queue);
    VkCommandPoolCreateInfo pci = {VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO};
    pci.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
    VkCommandBufferAllocateInfo cai = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO};
    cai.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    cai.commandBufferCount = 1;
    HostBuffer staging, upload;
    if (!resolved || !f.vkGetImageSubresourceLayout || !f.vkCmdSetScissor || !f.vkCmdSetViewport || !f.vkCmdCopyBufferToImage ||
        d.vkCreateCommandPool(d.device, &pci, nullptr, &d.pool) != VK_SUCCESS ||
        (cai.commandPool = d.pool, d.vkAllocateCommandBuffers(d.device, &cai, &d.cmd)) != VK_SUCCESS) {
        report("swap setup e5");
    } else if (staging.create(d, kWidth * kHeight * 4, VK_BUFFER_USAGE_TRANSFER_DST_BIT) != VK_SUCCESS ||
               upload.create(d, kWidth * kHeight * 4, VK_BUFFER_USAGE_TRANSFER_SRC_BIT) != VK_SUCCESS) {
        report("swap setup e6");
    } else {
        for (int kind = 0; kind < 2; ++kind) {
            if (kind == 1 && !hostImport) {report("swap host unsupported"); continue;}
            for (const char mode : {'s', 'g'}) {
                Target t;
                int error = makeTarget(d, f, kind == 1, t);
                if (!error) {
                    setMode(mode);
                    error = renderOnce(d, f, t, upload, staging);
                    setMode(0);
                }
                auto px = [&](uint32_t x, uint32_t y) {
                    static char out[4][8];
                    static int slot;
                    char *s = out[slot++ & 3];
                    const uint8_t *p = reinterpret_cast<const uint8_t *>(const_cast<const uint32_t *>(staging.words)) + (y * kWidth + x) * 4;
                    snprintf(s, 8, "%02x%02x%02x", p[0], p[1], p[2]);
                    return s;
                };
                snprintf(line, sizeof line, "swap %s%c A%s B%s C%s D%s p%u e%d", kind ? "host" : "dev", mode, px(508, 32), px(1524, 32),
                         px(2540, 16), px(2540, 48), unsigned(t.pitch), error);
                report(line);
                destroy(d, t);
            }
        }
        d.vkDeviceWaitIdle(d.device);
        staging.destroy(d);
        upload.destroy(d);
        d.vkDestroyCommandPool(d.device, d.pool, nullptr);
    }
    d.vkDestroyDevice(d.device, nullptr);
    destroyInstance(instance, nullptr);
}

}  // namespace swaptest
}  // namespace turnip
#pragma clang diagnostic pop
