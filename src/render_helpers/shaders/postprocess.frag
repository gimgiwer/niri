uniform float noise;
uniform float saturation;
uniform vec4 bg_color;

// Sin-less white noise by David Hoskins (MIT License).
// https://www.shadertoy.com/view/4djSRW
float hash12(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

vec3 saturate(vec3 color, float sat) {
    const vec3 w = vec3(0.2126, 0.7152, 0.0722);
    return mix(vec3(dot(color, w)), color, sat);
}

vec4 postprocess(vec4 color) {
    // Epsilon check skips saturation math when effectively identity to save ALU ops.
    if (abs(saturation - 1.0) > 0.001) {
        color.rgb = saturate(color.rgb, saturation);
    }

    // S-curve pivots at 0.45 to restore depth on blurred backgrounds without crushing shadows.
    const float contrast = 1.12;
    const float midpoint = 0.45;
    color.rgb = clamp((color.rgb - vec3(midpoint)) * contrast + vec3(midpoint), 0.0, 1.0);

    if (noise > 0.0) {
        vec2 uv = gl_FragCoord.xy;
        color.rgb += (hash12(uv) - 0.5) * noise;
    }

    // Mix bg_color behind the texture (both premultiplied alpha).
    color = color + bg_color * (1.0 - color.a);

    return color;
}
