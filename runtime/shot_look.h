#ifndef GRKIT_SHOT_LOOK_H
#define GRKIT_SHOT_LOOK_H
/* Byte-owned snapshot: no field points back into the mutable Look registry. */
typedef struct { u8 bytes[GRKIT_SHOT_LOOK_BYTES]; } ShotLook;
static u32 shot_look_u16(const ShotLook *look,u32 at) {
    return (u32)look->bytes[at]|((u32)look->bytes[at+1]<<8);
}
static u32 shot_look_u32(const ShotLook *look,u32 at) {
    return shot_look_u16(look,at)|(shot_look_u16(look,at+2)<<16);
}
/* Packed white is private core state in the SDK's reserved word. Bit 24
   distinguishes a black white point from a native shot's default white. */
static u32 shot_look_white(const ShotLook *look) {
    int rgb[3],v;
    u32 out=1U<<24,post=shot_look_u32(look,GRKIT_SHOT_LOOK_FEATURES)&GRKIT_FEATURE_POST;
    for(u32 c=0;c<3;++c) rgb[c]=(int)shot_look_u16(look,GRKIT_SHOT_LOOK_RGB+c*2);
    for(u32 k=0;k<3;++k) {
        v=rgb[k];
        if(post) {
            v=0;
            for(u32 c=0;c<3;++c) v+=(s16)shot_look_u16(look,GRKIT_SHOT_LOOK_POST+(k*3+c)*2)*rgb[c];
            v=(v+4096)>>13;
        }
        v=v<0 ? 0 : v>16383 ? 16383 : v;
        out|=((u32)v*255U+8192U)/16384U<<(k*8);
    }
    return out;
}
#endif
