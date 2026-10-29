// Eligible source-grid assembly. CPU-generated 16-bit texel coordinates retain
// the same half-open placement/coverage rules even at large canvas coordinates.
cbuffer DonorParameters : register(b0) {
    uint4 extent;      // output width/height, copy-leaf flag, reserved
    uint4 page;        // source page x/y/width/height
    uint4 area;        // output x/y/width/height
};
StructuredBuffer<uint> Pixels : register(t0);
StructuredBuffer<uint4> Axes : register(t1);
RWStructuredBuffer<uint> Result : register(u0);
RWTexture2D<float4> Display : register(u1);

uint4 unpack(uint p) { return uint4(p&255,(p>>8)&255,(p>>16)&255,p>>24); }
uint pack(uint4 p) { return p.x | (p.y<<8) | (p.z<<16) | (p.w<<24); }
uint read_pixel(uint x, uint y) { return Pixels[(y-page.y)*page.z+x-page.x]; }

uint round_byte(double v) {
    uint n=(uint)(float)v;
    if((double)n>v && n>0) n--;
    return min(255,n+(v-(double)n>=0.5 ? 1 : 0));
}
uint sample_pixel(uint4 x, uint4 y) {
    uint4 p[4]={unpack(read_pixel(x.x,y.x)),unpack(read_pixel(x.y,y.x)),
                unpack(read_pixel(x.x,y.y)),unpack(read_pixel(x.y,y.y))};
    double tx=(double)x.z/65536.0, ty=(double)y.z/65536.0;
    double weights[4]={(1.0-tx)*(1.0-ty),tx*(1.0-ty),(1.0-tx)*ty,tx*ty};
    precise double4 sum=0.0;
    [unroll] for(uint n=0;n<4;n++) {
        sum.xyz+=(double3)p[n].xyz*(double)p[n].w/255.0*weights[n];
        sum.w+=(double)p[n].w*weights[n];
    }
    uint a=round_byte(sum.w);
    return pack(uint4(a>0 ? uint3(round_byte(sum.x*255.0/sum.w),
        round_byte(sum.y*255.0/sum.w),round_byte(sum.z*255.0/sum.w)) : uint3(0,0,0),a));
}
float divide_f32(float a, float b) {
    // SM5 float division lowers to reciprocal/multiply. Round a double quotient
    // to f32 so byte truncation agrees with the CPU's correctly rounded divide.
    precise double result=(double)a/(double)b;
    if(a==0.0) return 0.0;
    float candidate=(float)result;
    uint bits=asuint(candidate);
    float lower=asfloat(bits-1), upper=asfloat(bits+1);
    // Double rounding at an f32 midpoint can still choose the wrong neighbor.
    // Products of a 25-bit midpoint and 24-bit divisor are exact in f64.
    precise double lo=((double)candidate+(double)lower)*0.5*(double)b;
    precise double hi=((double)candidate+(double)upper)*0.5*(double)b;
    if((double)a<lo || ((double)a==lo && (bits&1)!=0)) return lower;
    if((double)a>hi || ((double)a==hi && (bits&1)!=0)) return upper;
    return candidate;
}
uint blend_channel(uint p,uint q,float ba,float fa,float alpha) {
    precise float bg=divide_f32(p,255.0),fg=divide_f32(q,255.0);
    precise float bp=bg*ba,fp=fg*fa,inv=1.0-fa,rhs=bp*inv,premul=fp+rhs;
    precise float rgb=divide_f32(premul,alpha),value=255.0*rgb;
    return (uint)value;
}
uint blend_pixel(uint before, uint after) {
    uint4 p=unpack(before),q=unpack(after);
    if(q.w==0) return before;
    if(q.w==255) return after;
    // Keep the CPU's scalar evaluation and rounding before each byte truncation.
    precise float ba=divide_f32(p.w,255.0),fa=divide_f32(q.w,255.0);
    precise float sum=ba+fa,product=ba*fa,alpha=sum-product;
    return pack(uint4(blend_channel(p.x,q.x,ba,fa,alpha),blend_channel(p.y,q.y,ba,fa,alpha),
        blend_channel(p.z,q.z,ba,fa,alpha),(uint)(255.0*alpha)));
}
[numthreads(64,1,1)]
void assemble_donors(uint3 tid : SV_DispatchThreadID) {
    uint i=tid.x; if(i>=area.z*area.w) return;
    uint2 p=area.xy+uint2(i%area.z,i/area.z);
    uint4 x=Axes[p.x], y=Axes[extent.x+p.y];
    uint at=p.y*extent.x+p.x;
    uint pixel=sample_pixel(x,y);
    Result[at]=blend_pixel(Result[at],pixel);
}
[numthreads(64,1,1)]
void copy_donors(uint3 tid : SV_DispatchThreadID) {
    uint i=tid.x;if(i>=area.z*area.w)return;
    uint2 p=area.xy+uint2(i%area.z,i/area.z);
    Result[p.y*extent.x+p.x]=read_pixel(Axes[p.x].x,Axes[extent.x+p.y].x);
}
[numthreads(64,1,1)]
void present_donors(uint3 tid : SV_DispatchThreadID) {
    uint i=tid.x; if(i>=extent.x*extent.y) return;
    float4 p=(float4)unpack(Pixels[i])/255.0;
    Display[uint2(i%extent.x,i/extent.x)]=float4(p.xyz*p.w,p.w);
}
