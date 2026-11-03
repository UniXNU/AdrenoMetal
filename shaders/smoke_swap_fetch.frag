#version 450
// GMEM swap test: the attachment's red, as read back through the input attachment, written as grey (immune to any store swap).
layout(input_attachment_index = 0, set = 0, binding = 0) uniform subpassInput source;
layout(location = 0) out vec4 color;
void main() {
    const float red = subpassLoad(source).r;
    color = vec4(red, red, red, 1.0);
}
