#version 450

// Test fixture: writes the pushed color through unchanged, so the
// readback oracle compares against the exact bytes the test pushed.

layout(location = 0) in flat vec4 fragment_color;

layout(location = 0) out vec4 color;

void main() {
    color = fragment_color;
}
