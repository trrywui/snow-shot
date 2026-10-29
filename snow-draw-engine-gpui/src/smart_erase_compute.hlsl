// Source-grid reconstruction. Float buffers contain unassociated 0..255 RGB or
// Lab; only the final presentation textures contain premultiplied UNORM color.
cbuffer Parameters : register(b0) {
    uint4 extent;       // width, height, level, reserved
    uint4 previous;     // previous width/height, fine width/height
    uint4 options;      // target base, target count, pass, propagation/direction
    uint4 crop;         // left, top, width, height
    float4 scales;      // original search extent, synthesized weight, reserved
};
StructuredBuffer<float4> F0 : register(t0);
StructuredBuffer<float4> F1 : register(t1);
StructuredBuffer<float4> F2 : register(t2);
StructuredBuffer<float4> F3 : register(t3);
StructuredBuffer<uint> U0 : register(t4);
StructuredBuffer<uint> U1 : register(t5);
StructuredBuffer<uint> U2 : register(t6);
StructuredBuffer<uint> U3 : register(t7);
StructuredBuffer<uint> U4 : register(t8);
StructuredBuffer<uint> U5 : register(t9);
StructuredBuffer<uint> U6 : register(t10);
RWStructuredBuffer<float4> O0 : register(u0);
RWStructuredBuffer<float4> O1 : register(u1);
RWStructuredBuffer<uint> V0 : register(u2);
RWStructuredBuffer<uint> V1 : register(u3);
RWStructuredBuffer<uint> V2 : register(u4);
RWTexture2D<float4> Image : register(u5);
RWStructuredBuffer<uint> V3 : register(u6);
RWStructuredBuffer<uint> Meta : register(u7);

static const uint INVALID = 0xffffffff;
static const uint COVERED = 1, HOLE = 2, DOMAIN = 4;
uint metadata() { return 16 + extent.z * 12; }
bool skip_prepared_pyramid() {return Meta[14]==0 || Meta[11]!=0 || Meta[10]!=INVALID;}
uint count() { return extent.x * extent.y; }
bool inside(int2 p) { return all(p >= 0) && p.x < (int)extent.x && p.y < (int)extent.y; }
uint index_of(int2 p) { return p.y * extent.x + p.x; }
int2 position_of(uint i) { return int2(i % extent.x, i / extent.x); }
bool known(uint f) { return (f & (COVERED | HOLE)) == COVERED; }
float difference(float3 a, float3 b) { float3 d = a-b; return dot(d,d); }
uint random_next(inout uint state) {
    state ^= state << 13; state ^= state >> 17; state ^= state << 5; return state;
}
uint random_seed(uint i, uint pass_index) {
    uint s = 0x534e4f57 ^ (i+1)*0x9e3779b9 ^ (extent.z+1)*0x85ebca6b ^ (pass_index+1)*0xc2b2ae35;
    return s == 0 ? 1 : s;
}
float3 lab(float3 rgb) {
    float3 c = rgb / 255.0;
    float3 v = float3(c.x <= 0.04045 ? c.x/12.92 : pow((c.x+0.055)/1.055,2.4),
                      c.y <= 0.04045 ? c.y/12.92 : pow((c.y+0.055)/1.055,2.4),
                      c.z <= 0.04045 ? c.z/12.92 : pow((c.z+0.055)/1.055,2.4));
    float3 xyz = float3(dot(v,float3(0.4124564,0.3575761,0.1804375))/0.95047,
                        dot(v,float3(0.2126729,0.7151522,0.072175)),
                        dot(v,float3(0.0193339,0.119192,0.9503041))/1.08883);
    float3 f = float3(xyz.x>0.008856 ? pow(xyz.x,1.0/3.0) : 7.787*xyz.x+16.0/116.0,
                      xyz.y>0.008856 ? pow(xyz.y,1.0/3.0) : 7.787*xyz.y+16.0/116.0,
                      xyz.z>0.008856 ? pow(xyz.z,1.0/3.0) : 7.787*xyz.z+16.0/116.0);
    return float3(116*f.y-16,500*(f.x-f.y),200*(f.y-f.z));
}
float3 rgb_bytes(uint p) { return float3(p & 255, (p>>8)&255, (p>>16)&255); }

[numthreads(64,1,1)]
void load_source(uint3 tid : SV_DispatchThreadID) {
    if(options.w==2 && skip_prepared_pyramid())return;
    uint i=tid.x; if(i>=count()) return;
    uint f=U1[i]; V0[i]=f;
    O0[i]=float4(known(f) ? rgb_bytes(U0[i]) : 0.0,0.0);
}

// F0/U0: fine RGB/flags; O0/V0: coarse RGB/flags. Metadata holds active levels.
[numthreads(64,1,1)]
void downsample(uint3 tid : SV_DispatchThreadID) {
    if(options.w==2 && skip_prepared_pyramid())return;
    uint i=tid.x; if(i>=count()) return;
    if(Meta[metadata()-12]==0) { O0[i]=0; V0[i]=0; return; }
    int2 p=position_of(i)*2;
    bool covered=true, hole=false, domain=true;
    float3 sum=0; float total=0;
    static const float weights[5]={1,4,6,4,1};
    [loop] for(int y=-4;y<=4;y++) [loop] for(int x=-4;x<=4;x++) {
        int2 q=p+int2(x,y);
        bool valid=all(q>=0) && q.x<(int)previous.z && q.y<(int)previous.w;
        uint f=valid ? U0[q.y*previous.z+q.x] : 0;
        domain = domain && valid && (f&DOMAIN)!=0;
        if(abs(x)<=2 && abs(y)<=2) {
            covered = covered && valid && (f&COVERED)!=0;
            hole = hole || (f&HOLE)!=0;
            if(valid && known(f)) {
                float w=weights[x+2]*weights[y+2];
                sum+=F0[q.y*previous.z+q.x].rgb*w; total+=w;
            }
        }
    }
    V0[i]=(covered?COVERED:0) | (covered&&hole?HOLE:0) | (domain&&!hole?DOMAIN:0);
    O0[i]=float4(total>0 && !hole ? sum/total : 0.0,0.0);
}

// Each row owns disjoint storage. Deterministic row prefixes retain source-grid
// ordering; atomic append order never determines the random donor population.
// U0 flags -> V0 maximum valid radius + 1, V1 eight-word row statistics.
[numthreads(64,1,1)]
void erode_rows(uint3 tid : SV_DispatchThreadID) {
    if(options.w==1 && Meta[3]!=0) return;
    if(options.w==2 && skip_prepared_pyramid())return;
    uint y=tid.x; if(y>=extent.y) return;
    uint4 donors=0; uint holes=0, observations=0, minx=extent.x, maxx=0;
    for(uint x=0;x<extent.x;x++) {
        uint i=y*extent.x+x, f=U0[i], radius=0;
        observations+=known(f);
        if((f&HOLE)!=0) { holes++; minx=min(minx,x); maxx=max(maxx,x+1); }
        if((f&DOMAIN)!=0) {
            radius=1;
            [loop] for(int r=1;r<=3;r++) {
                bool safe=true;
                [loop] for(int dy=-r;dy<=r;dy++) [loop] for(int dx=-r;dx<=r;dx++) {
                    if(abs(dx)!=r && abs(dy)!=r) continue;
                    int2 p=int2(x,y)+int2(dx,dy);
                    if(!inside(p) || (U0[index_of(p)]&DOMAIN)==0) safe=false;
                }
                if(!safe) break;
                radius=r+1;
            }
        }
        V0[i]=radius;
        donors+=uint4(radius>0,radius>1,radius>2,radius>3);
    }
    for(uint r=0;r<4;r++) V1[y*8+r]=donors[r];
    V1[y*8+4]=holes; V1[y*8+5]=minx; V1[y*8+6]=maxx;
    V1[y*8+7]=observations;
}

[numthreads(1,1,1)]
void prefix_rows(uint3 tid : SV_DispatchThreadID) {
    if(options.w==2 && skip_prepared_pyramid())return;
    uint4 donors=0; uint holes=0, x0=extent.x,y0=extent.y,x1=0,y1=0;
    for(uint y=0;y<extent.y;y++) {
        donors+=uint4(U0[y*8],U0[y*8+1],U0[y*8+2],U0[y*8+3]);
        uint n=U0[y*8+4]; holes+=n;
        if(n>0) { x0=min(x0,U0[y*8+5]); x1=max(x1,U0[y*8+6]); y0=min(y0,y); y1=y+1; }
    }
    uint m=metadata();
    bool active=extent.z==0 || (Meta[m-12]!=0 && donors.w>=16 && holes>0);
    uint radius=donors.w>0 ? 3 : donors.z>0 ? 2 : donors.y>0 ? 1 : 0;
    Meta[m]=active; Meta[m+1]=holes; Meta[m+2]=donors[radius]; Meta[m+3]=radius;
    Meta[m+4]=donors.w; Meta[m+5]=clamp(min(x1-x0,y1-y0)/12,4,16);
    Meta[m+6]=x0; Meta[m+7]=y0; Meta[m+8]=x1; Meta[m+9]=y1;
    if(active) {
        Meta[0]=extent.z+1;
        if(holes==0 || donors.x==0) Meta[1]=1;
    }
    uint d=0,h=0;
    for(uint row=0;row<extent.y;row++) {
        V0[row*2]=d; V0[row*2+1]=h;
        d+=U0[row*8+radius]; h+=U0[row*8+4];
    }
}

// U0 radii, U1 row prefixes, U2 flags -> V0 donors, V1 donor mask, V2 targets.
[numthreads(64,1,1)]
void compact_rows(uint3 tid : SV_DispatchThreadID) {
    uint y=tid.x; if(y>=extent.y) return;
    uint d=U1[y*2], h=U1[y*2+1], radius=Meta[metadata()+3];
    for(uint x=0;x<extent.x;x++) {
        uint i=y*extent.x+x;
        bool donor=U0[i]>radius;
        V1[i]=donor;
        if(donor) V0[d++]=i;
        if((U2[i]&HOLE)!=0) V2[h++]=i;
    }
}

uint nearer(uint a,uint b,int2 p) {
    if(a==INVALID) return b; if(b==INVALID) return a;
    int2 aa=abs(position_of(a)-p), bb=abs(position_of(b)-p);
    uint da=aa.x+aa.y, db=bb.x+bb.y;
    return db<da || (db==da && b<a) ? b : a;
}
// Exact separable L1 transform; U0 seeds/row results -> V0 nearest seed indices.
[numthreads(64,1,1)]
void nearest_rows(uint3 tid : SV_DispatchThreadID) {
    uint y=tid.x; if(y>=extent.y) return; uint seed=INVALID;
    for(uint x=0;x<extent.x;x++) {
        uint i=y*extent.x+x; if(U0[i]!=0) seed=i; V0[i]=seed;
    }
    seed=INVALID;
    for(int xx=(int)extent.x-1;xx>=0;xx--) {
        uint i=y*extent.x+xx; if(U0[i]!=0) seed=i;
        V0[i]=nearer(V0[i],seed,int2(xx,y));
    }
}
[numthreads(64,1,1)]
void nearest_columns(uint3 tid : SV_DispatchThreadID) {
    uint x=tid.x; if(x>=extent.x) return; uint seed=INVALID;
    for(uint y=0;y<extent.y;y++) {
        uint i=y*extent.x+x; seed=nearer(U0[i],seed,int2(x,y)); V0[i]=seed;
    }
    seed=INVALID;
    for(int yy=(int)extent.y-1;yy>=0;yy--) {
        uint i=yy*extent.x+x; seed=nearer(V0[i],seed,int2(x,yy)); V0[i]=seed;
    }
}

[numthreads(64,1,1)]
void lab_colors(uint3 tid : SV_DispatchThreadID) {
    uint i=tid.x; if(i<count()) O0[i]=float4(lab(F0[i].rgb),0);
}
// F0 Lab + U0 flags -> O0 weighted first/second moments, V0 float weight bits.
[numthreads(64,1,1)]
void moments_horizontal(uint3 tid : SV_DispatchThreadID) {
    uint i=tid.x; if(i>=count()) return; int2 p=position_of(i);
    int r=Meta[metadata()+5]; float sigma=r/2.0; float4 sum=0; float weight=0;
    [loop] for(int k=-r;k<=r;k++) {
        uint j=p.y*extent.x+clamp(p.x+k,0,(int)extent.x-1);
        if(!known(U0[j])) continue;
        float w=exp(-float(k*k)/(2*sigma*sigma)); float3 v=F0[j].rgb;
        sum+=float4(v,dot(v,v))*w; weight+=w;
    }
    O0[i]=sum; V0[i]=asuint(weight);
}
[numthreads(64,1,1)]
void moments_vertical(uint3 tid : SV_DispatchThreadID) {
    uint i=tid.x; if(i>=count()) return; int2 p=position_of(i);
    int r=Meta[metadata()+5]; float sigma=r/2.0; float4 sum=0; float weight=0;
    [loop] for(int k=-r;k<=r;k++) {
        uint j=clamp(p.y+k,0,(int)extent.y-1)*extent.x+p.x;
        float w=exp(-float(k*k)/(2*sigma*sigma)); sum+=F0[j]*w; weight+=asfloat(U0[j])*w;
    }
    float3 mean=sum.rgb/max(weight,1e-6);
    float4 g=float4(mean,sqrt(max(0.0,sum.w/max(weight,1e-6)-dot(mean,mean))));
    O0[i]=g; O1[i]=g; V0[i]=asuint(3.402823466e+38);
}
// F0 immutable guide statistics, U0 flags -> O0 directional guides, V0 costs.
void flush_guide(int before,int after,int2 first,uint n,int2 direction) {
    if(n==0 || (before<0 && after<0)) return;
    uint a=before>=0?before:after, b=after>=0?after:before;
    float4 av=F0[a],bv=F0[b];
    float diff=difference(av.rgb,bv.rgb);
    float span=length(float2(position_of(a)-position_of(b)));
    float cost=diff+0.02*span+(before<0||after<0?1000.0:0.0);
    for(uint k=0;k<n;k++) {
        int2 p=first+direction*k; uint i=index_of(p);
        if(cost>=asfloat(V0[i])) continue;
        float t=span>0?length(float2(p-position_of(a)))/span:0;
        float3 color=diff<900?lerp(av.rgb,bv.rgb,t):(t<0.5?av.rgb:bv.rgb);
        O0[i]=float4(color,lerp(av.w,bv.w,t)); V0[i]=asuint(cost);
    }
}
[numthreads(64,1,1)]
void guide_lines(uint3 tid : SV_DispatchThreadID) {
    uint line_index=tid.x,dir=options.w; int2 p,step;
    if(dir==0) { if(line_index>=extent.y)return; p=int2(0,line_index); step=int2(1,0); }
    else if(dir==1) { if(line_index>=extent.x)return; p=int2(line_index,0); step=int2(0,1); }
    else {
        if(line_index>=extent.x+extent.y-1)return;
        step=dir==2?int2(1,1):int2(-1,1);
        p=line_index<extent.x?int2(line_index,0):int2(dir==2?0:extent.x-1,line_index-extent.x+1);
    }
    int before=-1; int2 first=0; uint pending=0;
    while(inside(p)) {
        uint i=index_of(p),f=U0[i];
        if(known(f)) { flush_guide(before,i,first,pending,step); pending=0; before=i; }
        else if((f&HOLE)!=0) { if(pending==0)first=p; pending++; }
        else { flush_guide(before,-1,first,pending,step); pending=0; before=-1; }
        p+=step;
    }
    flush_guide(before,-1,first,pending,step);
}

[numthreads(64,1,1)]
void initialize_matches(uint3 tid : SV_DispatchThreadID) {
    uint i=tid.x; if(i>=count())return;
    uint best=INVALID; float3 color=F0[i].rgb;
    if((U0[i]&HOLE)!=0) {
        best=U1[i]; float4 g=F3[i],b=F3[best];
        float error=difference(g.rgb,b.rgb)+4*(g.w-b.w)*(g.w-b.w);
        uint random=random_seed(i,0);
        [loop] for(uint k=0;k<32;k++) {
            uint candidate=U2[random_next(random)%Meta[metadata()+2]]; float4 v=F3[candidate];
            float score=difference(g.rgb,v.rgb)+4*(g.w-v.w)*(g.w-v.w);
            if(score<error) { best=candidate; error=score; }
        }
        color=F0[best].rgb;
        if(previous.x>0) {
            uint2 p=uint2(i%extent.x,i/extent.x)*previous.xy/extent.xy;
            uint j=p.y*previous.x+p.x, match=U3[j];
            if(match!=INVALID) {
                uint2 donor=uint2(match%previous.x,match/previous.x)*extent.xy/previous.xy;
                uint d=donor.y*extent.x+donor.x;
                if(U4[d]!=0)best=d;
                color=lab(F2[j].rgb);
            }
        }
    }
    V0[i]=best; V1[i]=INVALID; O0[i]=float4(color,0); O1[i]=float4(color,0);
}
[numthreads(64,1,1)]
void clear_reuse(uint3 tid : SV_DispatchThreadID) { if(tid.x<count())V0[tid.x]=0; }
[numthreads(64,1,1)]
void count_reuse(uint3 tid : SV_DispatchThreadID) {
    uint k=options.x+tid.x; if(tid.x>=options.y || k>=Meta[metadata()+1])return;
    uint i=U0[k]; InterlockedAdd(V0[U1[i]],1);
}

// Matching: F0 source Lab, F1 synthesized Lab, F2 guides; U0 flags, U1 prior
// matches, U2 donor list, U3 nearest, U4 reuse counts, U5 targets, U6 donor mask.
// V0 next matches, V1 float cost bits. Every propagation reads a frozen field.
void consider(uint i,uint p,inout uint best,inout float best_cost) {
    if(p==INVALID || U6[p]==0)return;
    int2 xy=position_of(i),dxy=position_of(p);
    float4 g=F2[i],d=F2[p];
    float expected=max(1.0,float(Meta[metadata()+1])/Meta[metadata()+2]);
    float reuse=25*log(1+float(U4[p])/expected);
    float fixed=8/(4+g.w)*difference(g.rgb,d.rgb)+4*(g.w-d.w)*(g.w-d.w)+reuse
        +8*dot(float2(dxy-xy),float2(dxy-xy))/(scales.x*scales.x);
    if(fixed>best_cost)return;
    float sum=0,norm=0; int radius=Meta[metadata()+3];
    [loop] for(int y=-radius;y<=radius;y++) [loop] for(int x=-radius;x<=radius;x++) {
        int2 q=xy+int2(x,y); if(!inside(q))continue;
        uint t=index_of(q),f=U0[t]; if((f&COVERED)==0)continue;
        uint s=index_of(dxy+int2(x,y)); float w=(f&HOLE)!=0?scales.y:1;
        sum+=difference(F1[t].rgb,F0[s].rgb)*w; norm+=w;
    }
    float score=sum/max(1.0,norm)+fixed;
    if(score<best_cost || (score==best_cost && p<best)) { best=p; best_cost=score; }
}
[numthreads(64,1,1)]
void match_patches(uint3 tid : SV_DispatchThreadID) {
    uint k=options.x+tid.x; if(tid.x>=options.y || k>=Meta[metadata()+1])return;
    uint i=U5[k],best=U1[i]; float cost=3.402823466e+38; uint random=random_seed(i,1+options.z*8+options.w);
    consider(i,best,best,cost); consider(i,U3[i],best,cost);
    consider(i,U2[random_next(random)%Meta[metadata()+2]],best,cost);
    int2 xy=position_of(i);
    static const int2 offsets[4]={int2(-1,0),int2(1,0),int2(0,-1),int2(0,1)};
    [unroll] for(uint n=0;n<4;n++) {
        int2 q=xy+offsets[n]; if(!inside(q))continue;
        uint p=U1[index_of(q)]; if(p==INVALID)continue;
        int2 candidate=position_of(p)-offsets[n];
        if(inside(candidate))consider(i,index_of(candidate),best,cost);
    }
    int2 center=position_of(U1[i]); int window=(int)scales.x;
    [loop] while(window>0) {
        int2 p=center+int2(random_next(random)%(window*2+1),random_next(random)%(window*2+1))-window;
        if(inside(p))consider(i,index_of(p),best,cost);
        window/=2;
    }
    V0[i]=best; V1[i]=asuint(cost);
}
[numthreads(64,1,1)]
void vote_patches(uint3 tid : SV_DispatchThreadID) {
    uint k=options.x+tid.x; if(tid.x>=options.y || k>=Meta[metadata()+1])return;
    uint i=U3[k]; int2 xy=position_of(i); int radius=Meta[metadata()+3];
    float3 anchor=F0[U1[i]].rgb; float tolerance=clamp(F2[i].w*0.25,1.5,10);
    float3 sum=0,rgb=0; float total=0;
    [loop] for(int y=-radius;y<=radius;y++) [loop] for(int x=-radius;x<=radius;x++) {
        int2 q=xy+int2(x,y); if(!inside(q))continue;
        uint n=index_of(q); if((U0[n]&HOLE)==0)continue;
        uint p=U1[n]; if(p==INVALID)continue;
        uint j=index_of(position_of(p)-int2(x,y));
        float w=1/((1+asfloat(U2[n]))*(1+difference(F2[j].rgb,F2[i].rgb)/100)
            *(1+difference(F0[j].rgb,anchor)/(tolerance*tolerance)));
        sum+=F0[j].rgb*w; rgb+=F1[j].rgb*w; total+=w;
    }
    O0[i]=float4(total>0?sum/total:F3[i].rgb,0);
    O1[i]=float4(total>0?rgb/total:F1[U1[i]].rgb,0);
}
// F0 filled RGB, U0 original straight RGBA, U1 flags -> V0/V1 exact straight
// original/reconstructed RGBA; Image is the separately premultiplied result.
[numthreads(64,1,1)]
void finish_pixels(uint3 tid : SV_DispatchThreadID) {
    uint i=tid.x; if(i>=crop.z*crop.w)return;
    uint2 xy=uint2(i%crop.z,i/crop.z),q=xy+crop.xy; uint j=q.y*extent.x+q.x;
    uint original=U0[j],pixel=original;
    if((U1[j]&HOLE)!=0) {
        uint packed;
        if(options.w==1) packed=previous.x;
        else if(options.w==2) packed=U0[U2[j]]&0x00ffffff;
        else {
            uint3 color=(uint3)clamp(floor(F0[j].rgb+0.5),0,255);
            packed=color.x | (color.y<<8) | (color.z<<16);
        }
        pixel=(original&0xff000000) | packed;
    }
    V0[i]=original; V1[i]=pixel;
    float a=float(pixel>>24)/255; Image[xy]=float4(rgb_bytes(pixel)/255*a,a);
}
[numthreads(64,1,1)]
void present_original(uint3 tid : SV_DispatchThreadID) {
    uint i=tid.x; if(i>=extent.x*extent.y)return; uint pixel=U0[i];
    float a=float(pixel>>24)/255;
    Image[uint2(i%extent.x,i/extent.x)]=float4(rgb_bytes(pixel)/255*a,a);
}

// Domain preparation uses a sixteen-word header: hole/known counts, reach,
// done, hole bbox, domain/radius-3 counts, periodic candidate, constant flag,
// known RGB min/max. No source image or pixel flags are copied to the CPU.
[numthreads(64,1,1)]
void source_flags(uint3 tid : SV_DispatchThreadID) {
    uint i=tid.x; if(i>=count()) return;
    uint p=U0[i]; bool covered=(p>>24)!=0, hole=covered && U1[i]!=0;
    uint f=(covered?COVERED:0)|(hole?HOLE:0);
    V0[i]=f; V1[i]=hole; V2[i]=f;
    if(covered && !hole) {
        uint ignored; InterlockedMin(Meta[12],p&0x00ffffff,ignored);
        InterlockedMax(Meta[13],p&0x00ffffff,ignored);
    }
}
[numthreads(1,1,1)]
void domain_initialize(uint3 tid : SV_DispatchThreadID) {
    uint holes=0,observations=0,x0=extent.x,y0=extent.y,x1=0,y1=0;
    for(uint y=0;y<extent.y;y++) {
        uint n=U0[y*8+4]; holes+=n; observations+=U0[y*8+7];
        if(n>0) {x0=min(x0,U0[y*8+5]);x1=max(x1,U0[y*8+6]);y0=min(y0,y);y1=y+1;}
    }
    Meta[0]=holes;Meta[1]=observations;Meta[2]=holes>0?clamp(min(x1-x0,y1-y0)/2,16,96):16;
    Meta[3]=holes==0;Meta[4]=x0;Meta[5]=y0;Meta[6]=x1;Meta[7]=y1;
    Meta[10]=INVALID;Meta[11]=observations>0 && Meta[12]==Meta[13];
    Meta[14]=holes;Meta[15]=observations;
}
[numthreads(64,1,1)]
void domain_pixels(uint3 tid : SV_DispatchThreadID) {
    uint i=tid.x;if(i>=count() || Meta[3]!=0)return;
    uint f=U0[i],seed=U1[i];int2 d=position_of(i)-position_of(seed);
    V0[i]=f|(known(f) && seed!=INVALID && uint(d.x*d.x+d.y*d.y)<=Meta[2]*Meta[2]?DOMAIN:0);
}
[numthreads(1,1,1)]
void domain_advance(uint3 tid : SV_DispatchThreadID) {
    if(Meta[3]!=0)return;
    uint donors=0,patches=0;
    for(uint y=0;y<extent.y;y++) {donors+=U0[y*8];patches+=U0[y*8+3];}
    Meta[8]=donors;Meta[9]=patches;
    if(patches>=64 || Meta[2]>=512)Meta[3]=1;
    else Meta[2]=min(Meta[2]*2,512);
}
[numthreads(1,1,1)]
void domain_margin(uint3 tid : SV_DispatchThreadID) {
    if(crop.z==0 || Meta[14]==0)return;
    uint margin=min(Meta[2]+16,512);
    uint2 lo=previous.xy+uint2(Meta[4],Meta[5]);
    lo=uint2(lo.x>margin?lo.x-margin:0,lo.y>margin?lo.y-margin:0);
    uint2 hi=min(previous.zw,previous.xy+uint2(Meta[6],Meta[7])+margin);
    if(any(lo<previous.xy) || any(hi>previous.xy+crop.xy))Meta[11]|=2;
}
groupshared float PeriodError[64];
groupshared uint PeriodCount[64];
groupshared uint DomainInfo[3];
groupshared uint TotalPairs;
groupshared float TotalError;
// One group per CPU-ordered candidate. Reject bad candidates in 4096-pixel
// batches; exact integer-valued costs below the acceptance limit sum losslessly.
[numthreads(64,1,1)]
void detect_periodic(uint3 group : SV_GroupID,uint3 lane : SV_GroupThreadID) {
    if(lane.x==0) {
        DomainInfo[0]=Meta[0];DomainInfo[1]=Meta[8];DomainInfo[2]=Meta[11];
        TotalPairs=0;TotalError=0;
    }
    GroupMemoryBarrierWithGroupSync();
    if(DomainInfo[0]==0 || DomainInfo[1]<64 || DomainInfo[2]!=0)return;
    uint candidate=group.x,period=candidate%63+2; bool horizontal=candidate<63;
    for(uint base=0;base<count();base+=4096) {
        uint pairs=0;float cost=0;
        for(uint k=0;k<64;k++) {
            uint i=base+k*64+lane.x;
            if(i<count() && (U1[i]&DOMAIN)!=0) {
                int2 q=position_of(i)+(horizontal?int2(period,0):int2(0,period));
                if(inside(q) && (U1[index_of(q)]&DOMAIN)!=0) {
                    pairs++;cost+=difference(rgb_bytes(U0[i]),rgb_bytes(U0[index_of(q)]));
                }
            }
        }
        PeriodCount[lane.x]=pairs;PeriodError[lane.x]=cost;GroupMemoryBarrierWithGroupSync();
        for(uint stride=32;stride>0;stride/=2) {
            if(lane.x<stride) {PeriodCount[lane.x]+=PeriodCount[lane.x+stride];PeriodError[lane.x]+=PeriodError[lane.x+stride];}
            GroupMemoryBarrierWithGroupSync();
        }
        if(lane.x==0) {TotalPairs+=PeriodCount[0];TotalError+=PeriodError[0];}
        GroupMemoryBarrierWithGroupSync();
        if(TotalError>float(DomainInfo[1])*0.5)return;
    }
    if(TotalPairs<64 || TotalError>float(TotalPairs)*0.5)return;
    uint lines=horizontal?extent.y:extent.x,span=horizontal?extent.x:extent.y;uint valid=1;
    for(uint task=lane.x;task<lines*period;task+=64) {
        uint line_index=task/period,residue=task%period;bool hole=false,donor=false;
        for(uint at=residue;at<span;at+=period) {
            uint i=horizontal?line_index*extent.x+at:at*extent.x+line_index;
            hole=hole || (U1[i]&HOLE)!=0;donor=donor || (U1[i]&DOMAIN)!=0;
        }
        if(hole && !donor) {valid=0;break;}
    }
    PeriodCount[lane.x]=valid;GroupMemoryBarrierWithGroupSync();
    for(uint stride=32;stride>0;stride/=2) {
        if(lane.x<stride)PeriodCount[lane.x]&=PeriodCount[lane.x+stride];
        GroupMemoryBarrierWithGroupSync();
    }
    if(lane.x==0 && PeriodCount[0]!=0) {uint ignored;InterlockedMin(Meta[10],candidate,ignored);}
}
// Exact nearest same-phase donor. Left/up wins ties, matching the CPU's search.
[numthreads(64,1,1)]
void periodic_lines(uint3 tid : SV_DispatchThreadID) {
    uint candidate=options.x,period=candidate%63+2;bool horizontal=candidate<63;
    uint lines=horizontal?extent.y:extent.x,span=horizontal?extent.x:extent.y;
    if(tid.x>=lines*period)return;
    uint line_index=tid.x/period,residue=tid.x%period,seed=INVALID;
    for(uint forward_index=residue;forward_index<span;forward_index+=period) {
        uint i=horizontal?line_index*extent.x+forward_index:forward_index*extent.x+line_index;
        if((U0[i]&DOMAIN)!=0)seed=i;V0[i]=seed;
    }
    seed=INVALID;int last=(int)residue+((int)span-1-(int)residue)/(int)period*(int)period;
    for(int at=last;at>=(int)residue;at-=(int)period) {
        uint i=horizontal?line_index*extent.x+at:at*extent.x+line_index;
        if((U0[i]&DOMAIN)!=0)seed=i;
        uint previous_seed=V0[i];
        if(previous_seed==INVALID)V0[i]=seed;
        else if(seed!=INVALID) {
            uint a=horizontal?previous_seed%extent.x:previous_seed/extent.x;
            uint b=horizontal?seed%extent.x:seed/extent.x;
            if(b-uint(at)<uint(at)-a)V0[i]=seed;
        }
    }
}
