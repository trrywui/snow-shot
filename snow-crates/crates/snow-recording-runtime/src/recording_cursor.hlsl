Texture2D<float4> desktop : register(t0);
Texture2D<float4> cursorImage : register(t1);
cbuffer Parameters : register(b0) {
    int2 outputSize;
    int2 origin;
    int2 scaledSize;
    int2 shapeSize;
    int masked;
    int highlightRadius;
    int2 reserved;
};

float4 vertex(uint id : SV_VertexID) : SV_Position {
    float2 corner = float2((id << 1) & 2, id & 2);
    return float4(corner * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
}

float4 pixel(float4 screen : SV_Position) : SV_Target {
    int2 coordinate = int2(screen.xy);
    uint4 background = (uint4)round(saturate(desktop.Load(int3(coordinate, 0))) * 255.0);
    int2 position = coordinate - origin;
    if (masked == 2) {
        float2 delta = float2(position) + 0.5;
        uint alpha = (uint)round(shapeSize.y * saturate(highlightRadius + 0.5 - length(delta)));
        if (all(position >= -highlightRadius) && all(position < highlightRadius)) {
            uint3 color = uint3(scaledSize, shapeSize.x);
            uint3 factor = 255 * (255 - alpha) + alpha * color;
            background.rgb = (background.rgb * factor + 32512) / 65025;
        }
    } else if (all(position >= 0) && all(position < scaledSize)) {
        int2 source = position * shapeSize / scaledSize;
        uint4 pixel = (uint4)round(saturate(cursorImage.Load(int3(source, 0))) * 255.0);
        if (masked != 0 && (pixel.a == 0 || pixel.a == 255)) {
            // Windows masked-color alpha is an AND mask, not opacity. Preserve
            // bytewise XOR, including #7f -> #80; there is no documented gray clamp.
            // https://learn.microsoft.com/windows-hardware/drivers/ddi/d3dkmddi/ns-d3dkmddi-_dxgkarg_setpointershape
            background.rgb = (background.rgb & pixel.aaa) ^ pixel.rgb;
        } else {
            background.rgb = (pixel.rgb * pixel.a + background.rgb * (255 - pixel.a) + 127) / 255;
        }
    }
    return float4(background.rgb / 255.0, 1.0);
}
