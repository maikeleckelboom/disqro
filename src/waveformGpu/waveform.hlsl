// Colours are encoded 8-bit display codes. The target and GPUI compositor
// both use UNORM views, so coverage is evaluated in the reference colour space.
cbuffer View : register(b0) {
    uint4 extent; // width, height, presentation, atlas mask
    float4 background;
    float4 stroke; // axis opacity, neutral edge, gain, intensity
    uint4 detail;  // cell count, first source slice, first spectral page, end of descriptors
    uint4 source;  // last spectral hop, light theme, preview bin count, generic palette offset
    float4 preview; // first bin, bins per pixel, end bin, reserved
    float4 samples; // source frame at left edge, frames per pixel, detail blend, reserved
    uint4 sample_tiles; // first and end tile descriptors
};

StructuredBuffer<uint4> columns : register(t0);

float4 vertex(uint id : SV_VertexID) : SV_Position {
    float2 corner = float2((id << 1) & 2, id & 2);
    return float4(corner * float2(2, -2) + float2(-1, 1), 0, 1);
}

// Preparation dispatches run once per display cell and column. The pixel
// pass above reads their output for each device row. Source buffers outlive
// each viewport and are shared by every visible panel on the device.
StructuredBuffer<uint4> projection : register(t1);
StructuredBuffer<uint4> evidence : register(t2);
StructuredBuffer<uint4> cores : register(t3);
StructuredBuffer<uint4> overview_bins : register(t4);
RWStructuredBuffer<uint4> prepared : register(u0);

static const uint NO_WITNESS = 0xffffffff;

void include_sample(inout float2 values, inout uint2 witnesses, float value, uint frame) {
    if (value < values.x || (value == values.x && frame < witnesses.x)) {
        values.x = value;
        witnesses.x = frame;
    }
    if (value > values.y || (value == values.y && frame < witnesses.y)) {
        values.y = value;
        witnesses.y = frame;
    }
}

[numthreads(64, 1, 1)]
void stroke_cores(uint cell : SV_DispatchThreadID) {
    if (cell >= detail.x) return;
    uint2 span = projection[extent.x + cell].xy;
    if (span.x == span.y) {
        prepared[cell] = 0;
        return;
    }
    int reached = int(span.x);
    float infinity = asfloat(0x7f800000);
    float2 bounds = float2(infinity, -infinity);
    uint2 witness = NO_WITNESS;
    bool silent = true;
    for (uint i = detail.y; i < detail.z && reached < int(span.y); i += 2) {
        int4 slice = asint(projection[i]); // owned start/end, leaf origin, leaf width
        uint4 page = projection[i + 1];  // evidence offset, leaf stride, page start/end
        if (slice.y <= reached) continue;
        if (slice.x > reached) break;
        int end = min(slice.y, int(span.y));
        for (int leaf = (reached - slice.z) / slice.w; reached < end; ++leaf) {
            int base = slice.z + leaf * slice.w;
            int until = min(base + slice.w, end);
            uint address = page.x + uint(leaf) * page.y;
            uint4 item = evidence[address];
            float2 values = asfloat(item.xy);
            int2 frames = base + int2(item.zw);
            if (values.x == values.y) include_sample(bounds, witness, values.x, reached);
            if (frames.x >= reached && frames.x < until) include_sample(bounds, witness, values.x, frames.x);
            if (frames.y >= reached && frames.y < until) include_sample(bounds, witness, values.y, frames.y);
            if (page.y == 2) {
                float4 endpoints = asfloat(evidence[address + 1]);
                if (reached == max(base, int(page.z)) && reached >= slice.x) {
                    include_sample(bounds, witness, endpoints.x, reached);
                    include_sample(bounds, witness, endpoints.y, reached);
                }
                if (until == min(base + slice.w, int(page.w)) && until <= slice.y) {
                    include_sample(bounds, witness, endpoints.z, until - 1);
                    include_sample(bounds, witness, endpoints.w, until - 1);
                }
            }
            silent = silent && all(values == 0);
            reached = until;
        }
    }
    bool held = reached == int(span.y);
    bool measured = held && witness.x != NO_WITNESS && (silent || any(bounds != 0));
    prepared[cell] = measured ? uint4(asuint(bounds), witness) : uint4(0, 0, NO_WITNESS, held);
}

static const float3 colour_weights[24] = {
    float3(30.995394, -0.90819, -0.263928), float3(19.333413, -3.313851, -1.72278),
    float3(8.395741, 18.934976, -2.852861), float3(-4.617554, 21.417598, -11.604659),
    float3(1.173827, 21.35068, 8.702214), float3(3.34275, 2.422165, 26.569969),
    float3(1.012681, -14.418055, 26.844175), float3(-7.485634, -3.082375, 75.728855),
    float3(90.542327, 1.774565, -0.279993), float3(56.303194, 0.511826, 1.085057),
    float3(26.758376, 57.509247, -5.187386), float3(-1.415483, 87.626811, 5.191878),
    float3(-2.230263, 51.469368, 43.142446), float3(1.362431, 2.420432, 30.67431),
    float3(-7.49192, 11.576582, 86.072166), float3(-2.14454, -2.154514, 81.434454),
    float3(33.423589, 2.212453, 1.15457), float3(20.565285, 4.681162, 5.084175),
    float3(14.580796, 27.770322, 2.834312), float3(4.626897, 35.411006, 10.081777),
    float3(6.248015, 19.199546, 1.061621), float3(-0.503712, 2.548138, 31.552237),
    float3(12.776072, -1.836481, 34.303679), float3(-5.345312, 8.354502, 53.149336)
};

bool spectrum_address(uint hop, out uint address) {
    for (uint i = detail.w; i > detail.z; --i) {
        uint3 page = projection[i - 1].xyz;
        if (hop >= page.x && hop < page.y) {
            address = page.z + 2 * (hop - page.x);
            return true;
        }
    }
    address = 0;
    return false;
}

bool folded(int hop, out float3 rgb) {
    rgb = 0;
    [unroll] for (int lag = 0; lag < 3; ++lag) {
        uint address;
        if (!spectrum_address(clamp(max(0, hop) + lag - 1, 0, int(source.x)), address)) return false;
        float4 a = asfloat(evidence[address]);
        float4 b = asfloat(evidence[address + 1]);
        [unroll] for (uint band = 0; band < 4; ++band) {
            rgb += a[band] * colour_weights[lag * 8 + band];
            rgb += b[band] * colour_weights[lag * 8 + band + 4];
        }
    }
    return true;
}

float2 stroke_y(float2 sample) {
    float h = float(extent.y);
    return h * 0.5 - clamp(sample * stroke.z, -1, 1) * (h * 0.46);
}

bool stroke_palette(uint x, uint2 position, out float3 rgb) {
    if (source.w != 0) {
        rgb = asfloat(projection[source.w + x]).rgb;
        return rgb.x >= 0;
    }
    rgb = 0;
    float3 left, right;
    if (!folded(asint(position.x), left) || !folded(asint(position.x) + 1, right)) return false;
    rgb = clamp(lerp(left, right, asfloat(position.y)), 40, 255);
    float neutral = source.y ? 90 : 150;
    float contrast = source.y ? min(0.72, 105.0 / max(1.0, dot(rgb, float3(0.2126, 0.7152, 0.0722)))) : 1.0;
    rgb = clamp(neutral + (rgb * contrast - neutral) * stroke.w, 0, 255);
    return true;
}

bool source_sample(int frame, out float2 stereo) {
    for (uint i = sample_tiles.x; i < sample_tiles.y; ++i) {
        uint3 tile = projection[i].xyz;
        if (frame >= int(tile.x) && frame < int(tile.y)) {
            stereo = asfloat(evidence[tile.z + uint(frame) - tile.x].xy);
            return true;
        }
    }
    stereo = 0;
    return false;
}

static const uint MAX_SEGMENTS = 9;

// Source lookup and segment setup run once per device column. Most pixel
// rows lie outside this column's vertical trace range and skip its segments.
void prepare_samples(uint x) {
    if (samples.z <= 0 || sample_tiles.x == sample_tiles.y) return;
    float frame = samples.x + (float(x) + 0.5) * samples.y;
    int first = max(0, int(floor(frame - samples.y))) - 1;
    int last = int(ceil(frame + samples.y));
    uint address = 3 * extent.x + x * 2 * MAX_SEGMENTS;
    float y_min = float(extent.y);
    float y_max = 0;
    uint count = 0;
    for (int index = first; index <= last; ++index) {
        float2 from, to;
        bool a = source_sample(index, from);
        bool b = source_sample(index + 1, to);
        if (!a && !b) continue;
        float x0 = (float(index) - frame) / samples.y;
        float x1 = x0 + rcp(samples.y);
        float2 y0 = stroke_y(from);
        float2 y1 = stroke_y(to);
        if (!a) { x0 = x1; y0 = y1; }
        if (!b) { x1 = x0; y1 = y0; }
        y_min = min(y_min, min(min(y0.x, y0.y), min(y1.x, y1.y)));
        y_max = max(y_max, max(max(y0.x, y0.y), max(y1.x, y1.y)));
        float dx = x1 - x0;
        float2 dy = y1 - y0;
        float2 inverse_length = rcp(max(dx * dx + dy * dy, 1e-6));
        prepared[address + 2 * count] = asuint(float4(x0, x1, y0));
        prepared[address + 2 * count + 1] = asuint(float4(y1, inverse_length));
        if (++count == MAX_SEGMENTS) break;
    }
    prepared[2 * extent.x + x] = uint4(asuint(y_min), asuint(y_max), count, 0);
}

[numthreads(64, 1, 1)]
void stroke_columns(uint x : SV_DispatchThreadID) {
    if (x >= extent.x) return;
    prepare_samples(x);
    uint4 inputs = projection[x]; // cell index, octave hop, fraction, EOF coverage
    float q = asfloat(inputs.w);
    uint4 core = cores[inputs.x];
    float2 bounds = asfloat(core.xy);
    prepared[2 * x] = 0;
    prepared[2 * x + 1] = uint4(0, 0, 0, asuint(-1.0));
    if (q <= 0) return;
    if (core.z == NO_WITNESS || all(bounds == 0)) {
        float state = core.z == NO_WITNESS ? (core.w != 0 ? -2.0 : -1.0) : 0.0;
        float3 ink = 0;
        if (samples.z > 0) {
            if (!stroke_palette(x, inputs.yz, ink)) ink = 0;
        }
        prepared[2 * x + 1] = asuint(float4(ink, state));
        return;
    }
    float3 rgb;
    if (!stroke_palette(x, inputs.yz, rgb)) return;
    uint4 before = cores[inputs.x - 1];
    uint4 after = cores[inputs.x + 1];
    float2 a = before.z == NO_WITNESS ? bounds : asfloat(before.xy);
    float2 b = after.z == NO_WITNESS ? bounds : asfloat(after.xy);
    float2 y = stroke_y(bounds.yx);
    if (y.y - y.x < 2) y = (y.x + y.y) * 0.5 + float2(-1, 1);
    float2 outer = stroke_y(float2(max(bounds.y, max(a.y, b.y)), min(bounds.x, min(a.x, b.x))));
    prepared[2 * x] = asuint(float4(y, min(y.x, outer.x), max(y.y, outer.y)));
    prepared[2 * x + 1] = asuint(float4(rgb, q));
}

float ramp(float u) {
    return u * u - u * u * u / 3.0;
}

float fringe(float row, float top, float bottom, bool reverse) {
    float a = max(row, top);
    float b = min(row + 1.0, bottom);
    if (b <= a) return 0;
    float span = bottom - top;
    if (reverse) return span * (ramp((bottom - a) / span) - ramp((bottom - b) / span));
    return span * (ramp((b - top) / span) - ramp((a - top) / span));
}

float3 coarse_colour(uint2 pixel) {
    if (source.z == 0) return background.rgb;
    float coordinate = preview.x + (float(pixel.x) + 0.5) * preview.y;
    if (coordinate < 0 || coordinate >= preview.z) return background.rgb;
    float3 column = asfloat(overview_bins[min(uint(coordinate), source.z - 1)]).xyz;
    float2 extremes = float2(max(0, column.y), min(0, column.x));
    float2 body = clamp(extremes, -column.z * 1.41421356, column.z * 1.41421356);
    float2 peak_y = stroke_y(extremes);
    float2 body_y = stroke_y(body);
    float y = float(pixel.y);
    float peak = max(0, min(y + 1, peak_y.y) - max(y, peak_y.x));
    float rms = max(0, min(y + 1, body_y.y) - max(y, body_y.x));
    return lerp(background.rgb, float3(150, 168, 180), 0.60 * rms + 0.28 * (peak - rms));
}

float3 stroke_colour(uint2 pixel, float4 colour) {
    float4 core = asfloat(columns[pixel.x * 2]);
    float q = colour.a;
    if (q < 0) return coarse_colour(pixel);
    float y = float(pixel.y);
    float solid = max(0, min(y + 1.0, core.y) - max(y, core.x));
    float a = min(1, fringe(y, core.z, core.x, false) + solid + fringe(y, core.y, core.w, true));
    if (a * q > 0) {
        return background.rgb * (1.0 - a * q) + q * (a * colour.rgb + stroke.y * a * (1 - a));
    }
    return pixel.y == extent.y / 2
        ? lerp(background.rgb, float3(150, 168, 180), stroke.x) : background.rgb;
}

float distance_squared(float2 a, float2 edge, float inverse_length) {
    float along = saturate(-dot(a, edge) * inverse_length);
    float2 closest = a + edge * along;
    return dot(closest, closest);
}

float3 sample_colour(float2 pixel, float3 envelope, float4 colour) {
    if (samples.z <= 0 || sample_tiles.x == sample_tiles.y) return envelope;
    uint x = uint(pixel.x);
    uint4 summary = columns[2 * extent.x + x];
    if (summary.z == 0) return envelope;
    float distance = 4.0;
    float2 limits = asfloat(summary.xy);
    if (pixel.y >= limits.x - 1.1 && pixel.y <= limits.y + 1.1) {
        uint address = 3 * extent.x + x * 2 * MAX_SEGMENTS;
        for (uint i = 0; i < summary.z; ++i) {
            float4 a = asfloat(columns[address + 2 * i]);
            float4 b = asfloat(columns[address + 2 * i + 1]);
            float2 y0 = a.zw - pixel.y;
            float2 y1 = b.xy - pixel.y;
            float2 dy = y1 - y0;
            float dx = a.y - a.x;
            if (min(y0.x, y1.x) <= 1.1 && max(y0.x, y1.x) >= -1.1) {
                distance = min(distance, distance_squared(float2(a.x, y0.x), float2(dx, dy.x), b.z));
            }
            if (min(y0.y, y1.y) <= 1.1 && max(y0.y, y1.y) >= -1.1) {
                distance = min(distance, distance_squared(float2(a.x, y0.y), float2(dx, dy.y), b.w));
            }
        }
    }
    float3 ink = any(colour.rgb > 0) ? colour.rgb : float3(150, 168, 180);
    float3 trace = lerp(background.rgb, ink, saturate(1.1 - sqrt(distance)));
    return lerp(envelope, trace, samples.z);
}

uint4 covered(uint4 rows, uint y) {
    return uint4(y >= (rows & 0xffff)) & uint4(y < (rows >> 16));
}

float average(uint4 samples) {
    return float(samples.x + samples.y + samples.z + samples.w) * 0.25;
}

float3 overview_colour(uint2 pixel) {
    uint4 peaks = covered(columns[pixel.x * 3], pixel.y);
    uint4 body = covered(columns[pixel.x * 3 + 1], pixel.y);
    float filled = average(body);
    float cover = filled + 0.65 * (extent.w ? average(peaks) * (1 - filled) : average(peaks & ~body));
    return lerp(background.rgb, asfloat(columns[pixel.x * 3 + 2]).rgb, cover);
}

float4 fragment(float4 position : SV_Position) : SV_Target {
    uint2 pixel = uint2(position.xy);
    float3 colour = background.rgb;
    switch (extent.z) {
        case 1: {
            float4 stroke_ink = asfloat(columns[pixel.x * 2 + 1]);
            colour = sample_colour(position.xy, stroke_colour(pixel, stroke_ink), stroke_ink);
            break;
        }
        case 3: colour = overview_colour(pixel); break;
    }
    colour = floor(colour + 0.5);
    return float4(colour / 255.0, 1);
}
