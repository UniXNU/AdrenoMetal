// SPDX-License-Identifier: MIT
// Two-plane YCbCr IOSurface conversion kernel (NativeMetalYCbCr.rs).
// Converts 8-bit/10-bit unpacked and 10-bit packed 4:2:0/4:2:2/4:4:4 YCbCr planes
// in an imported IOSurface buffer directly into an RGBA16Float texture using Metal's
// chroma filter (9/3/3/1 for 4:2:0, 3/1 for 4:2:2) and fixed-point YCbCr conversion.
#include <metal_stdlib>
using namespace metal;

struct YCbCrParams {
    uint width;
    uint height;
    uint luma_offset;
    uint luma_row_bytes;
    uint chroma_offset;
    uint chroma_row_bytes;
    uint chroma_w;
    uint chroma_h;
    uint flags;
    int kb_r;
    int kr_r;
    int kb_g;
    int kr_g;
    int kb_b;
    int kr_b;
    uint _pad;
};

static __attribute__((always_inline)) inline uint load_u8(device const uint *src, uint addr) {
    return (src[addr >> 2u] >> ((addr & 3u) * 8u)) & 0xFFu;
}

static __attribute__((always_inline)) inline uint load_u16(device const uint *src, uint addr) {
    if ((addr & 1u) == 0u) {
        return (src[addr >> 2u] >> ((addr & 2u) * 8u)) & 0xFFFFu;
    }
    return load_u8(src, addr) | (load_u8(src, addr + 1u) << 8u);
}

static __attribute__((always_inline)) inline uint load_u32(device const uint *src, uint addr) {
    if ((addr & 3u) == 0u) {
        return src[addr >> 2u];
    }
    uint lo = src[addr >> 2u];
    uint hi = src[(addr >> 2u) + 1u];
    uint sh = (addr & 3u) * 8u;
    return (lo >> sh) | (hi << (32u - sh));
}

static __attribute__((always_inline)) inline uint read_code(device const uint *src, uint row_base, uint index, uint bits_mode) {
    if (bits_mode == 0u) {
        return load_u8(src, row_base + index);
    }
    if (bits_mode == 1u) {
        return load_u16(src, row_base + (index << 1u)) >> 6u;
    }
    uint word = load_u32(src, row_base + (index / 3u) * 4u);
    return (word >> ((index % 3u) * 10u)) & 0x3FFu;
}

static __attribute__((always_inline)) inline uint2 read_chroma_pair(device const uint *src, uint chroma_offset, uint chroma_row_bytes,
                                     uint cx, uint cy, uint bits_mode) {
    uint row = chroma_offset + cy * chroma_row_bytes;
    uint base = cx << 1u;
    return uint2(read_code(src, row, base, bits_mode),
                 read_code(src, row, base + 1u, bits_mode));
}

static __attribute__((always_inline)) inline float srgb_to_linear(float c) {
    return (c <= 0.04045f) ? (c / 12.92f) : pow((c + 0.055f) / 1.055f, 2.4f);
}

static __attribute__((always_inline)) inline float pq_to_linear(float e) {
    if (e <= 0.0f) {
        return 0.0f;
    }
    const float m1 = 0.1593017578125f;
    const float m2 = 78.84375f;
    const float c1 = 0.8359375f;
    const float c2 = 18.8515625f;
    const float c3 = 18.6875f;
    float p = pow(e, 1.0f / m2);
    float num = p - c1;
    if (num <= 0.0f) {
        return 0.0f;
    }
    float den = max(c2 - c3 * p, 1e-6f);
    return pow(num / den, 1.0f / m1);
}

static __attribute__((always_inline)) inline float hlg_to_linear(float e) {
    const float a = 0.17883277f;
    const float b = 0.28466892f;
    const float c = 0.55991073f;
    if (e <= 0.5f) {
        return (e * e) / 3.0f;
    }
    return (exp((e - c) / a) + b) / 12.0f;
}

kernel void ycbcr_convert(
    device const uint *src [[buffer(0)]],
    constant YCbCrParams &p [[buffer(1)]],
    texture2d<float, access::write> dst [[texture(0)]],
    uint2 gid [[thread_position_in_grid]])
{
    if (gid.x >= p.width || gid.y >= p.height) {
        return;
    }
    uint bits_mode = p.flags & 3u;
    uint subsampling = (p.flags >> 2u) & 3u;
    uint range = (p.flags >> 4u) & 1u;
    uint output = (p.flags >> 5u) & 7u;

    uint y_code = read_code(src, p.luma_offset + gid.y * p.luma_row_bytes, gid.x, bits_mode);

    uint2 cbcr;
    if (subsampling == 0u) {
        uint cx0 = min(gid.x >> 1u, p.chroma_w - 1u);
        uint cx1 = (gid.x & 1u) != 0u ? min(cx0 + 1u, p.chroma_w - 1u) : (cx0 > 0u ? cx0 - 1u : 0u);
        uint cy0 = min(gid.y >> 1u, p.chroma_h - 1u);
        uint cy1 = (gid.y & 1u) != 0u ? min(cy0 + 1u, p.chroma_h - 1u) : (cy0 > 0u ? cy0 - 1u : 0u);
        uint2 c00 = read_chroma_pair(src, p.chroma_offset, p.chroma_row_bytes, cx0, cy0, bits_mode);
        uint2 c10 = read_chroma_pair(src, p.chroma_offset, p.chroma_row_bytes, cx1, cy0, bits_mode);
        uint2 c01 = read_chroma_pair(src, p.chroma_offset, p.chroma_row_bytes, cx0, cy1, bits_mode);
        uint2 c11 = read_chroma_pair(src, p.chroma_offset, p.chroma_row_bytes, cx1, cy1, bits_mode);
        cbcr = (9u * c00 + 3u * c10 + 3u * c01 + c11 + 8u) >> 4u;
    } else if (subsampling == 1u) {
        uint cx0 = min(gid.x >> 1u, p.chroma_w - 1u);
        uint cx1 = (gid.x & 1u) != 0u ? min(cx0 + 1u, p.chroma_w - 1u) : (cx0 > 0u ? cx0 - 1u : 0u);
        uint cy0 = min(gid.y, p.chroma_h - 1u);
        uint2 c0 = read_chroma_pair(src, p.chroma_offset, p.chroma_row_bytes, cx0, cy0, bits_mode);
        uint2 c1 = read_chroma_pair(src, p.chroma_offset, p.chroma_row_bytes, cx1, cy0, bits_mode);
        cbcr = (3u * c0 + c1 + 2u) >> 2u;
    } else {
        uint cx0 = min(gid.x, p.chroma_w - 1u);
        uint cy0 = min(gid.y, p.chroma_h - 1u);
        cbcr = read_chroma_pair(src, p.chroma_offset, p.chroma_row_bytes, cx0, cy0, bits_mode);
    }

    uint bits = (bits_mode == 0u) ? 8u : 10u;
    uint max_code = (1u << bits) - 1u;
    float3 rgb;

    if (output == 2u) {
        float inv = 1.0f / float(max_code);
        rgb = float3(float(cbcr.y) * inv, float(y_code) * inv, float(cbcr.x) * inv);
    } else {
        uint up = 10u - bits;
        int luma = (range == 0u)
            ? (9576 * (int(y_code << up) - 64))
            : (256 * int((y_code * 32768u + (max_code >> 1u)) / max_code));
        int cb_s = int(cbcr.x << up) - 512;
        int cr_s = int(cbcr.y << up) - 512;
        int tr = luma + p.kb_r * cb_s + p.kr_r * cr_s;
        int tg = luma + p.kb_g * cb_s + p.kr_g * cr_s;
        int tb = luma + p.kb_b * cb_s + p.kr_b * cr_s;
        if (output == 0u) {
            rgb = float3(
                float(clamp((tr + 2048) >> 12, 0, 2048)) * (1.0f / 2048.0f),
                float(clamp((tg + 2048) >> 12, 0, 2048)) * (1.0f / 2048.0f),
                float(clamp((tb + 2048) >> 12, 0, 2048)) * (1.0f / 2048.0f));
        } else if (output == 1u) {
            float r = float(clamp((tr + 512) >> 10, 0, 8192)) * (1.0f / 8192.0f);
            float g = float(clamp((tg + 512) >> 10, 0, 8192)) * (1.0f / 8192.0f);
            float b = float(clamp((tb + 512) >> 10, 0, 8192)) * (1.0f / 8192.0f);
            rgb = float3(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b));
        } else if (output == 3u) {
            rgb = float3(
                float((tr + 2048) >> 12) * (1.0f / 2048.0f),
                float((tg + 2048) >> 12) * (1.0f / 2048.0f),
                float((tb + 2048) >> 12) * (1.0f / 2048.0f));
        } else if (output == 4u) {
            float r = float(clamp((tr + 512) >> 10, 0, 8192)) * (1.0f / 8192.0f);
            float g = float(clamp((tg + 512) >> 10, 0, 8192)) * (1.0f / 8192.0f);
            float b = float(clamp((tb + 512) >> 10, 0, 8192)) * (1.0f / 8192.0f);
            rgb = float3(pq_to_linear(r), pq_to_linear(g), pq_to_linear(b));
        } else {
            float r = float(clamp((tr + 512) >> 10, 0, 8192)) * (1.0f / 8192.0f);
            float g = float(clamp((tg + 512) >> 10, 0, 8192)) * (1.0f / 8192.0f);
            float b = float(clamp((tb + 512) >> 10, 0, 8192)) * (1.0f / 8192.0f);
            rgb = float3(hlg_to_linear(r), hlg_to_linear(g), hlg_to_linear(b));
        }
    }

    dst.write(float4(rgb, 1.0f), gid);
}
