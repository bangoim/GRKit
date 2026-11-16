#ifndef GRKIT_EFFECT_RECORD_H
#define GRKIT_EFFECT_RECORD_H
#include "abi.h"
/* Build a private record from a validated GRF. Nothing is published here.
   Addresses describe the final slot, so moving the result needs rebuilding.
   The caller adds multiaxial/toning data for native or hue-aware profiles. */
static u32 effect_record_word(const u8 *p) {
    return (u32)p[0]|((u32)p[1]<<8)|((u32)p[2]<<16)|((u32)p[3]<<24);
}
static void effect_record_store(u8 *p,u32 v) {
    p[0]=(u8)v; p[1]=(u8)(v>>8); p[2]=(u8)(v>>16); p[3]=(u8)(v>>24);
}
static void effect_record_copy(u8 *to,const u8 *from,u32 n) {
    for(u32 i=0;i<n;++i) to[i]=from[i];
}
static void effect_record_prepare(u8 *out,const u8 *stock,const u8 *profile,
                                  u32 address,u32 icon,u32 code) {
    u32 features=effect_record_word(profile+GRKIT_GRF_FEATURES);
    for(u32 i=0;i<GRKIT_RECORD_SIZE;++i) ((volatile u8 *)out)[i]=0;
    effect_record_copy(out,stock,GRKIT_RECORD_COEFFICIENTS);
    effect_record_store(out+4,address+GRKIT_RECORD_COEFFICIENTS);
    effect_record_copy(out+GRKIT_RECORD_COEFFICIENTS,profile+GRKIT_GRF_MATRIX,GRKIT_GRF_ICON-GRKIT_GRF_MATRIX);
    effect_record_copy(out+GRKIT_RECORD_FEATURES,profile+GRKIT_GRF_FEATURES,GRKIT_GRF_FEATURES_SIZE);
    effect_record_copy(out+GRKIT_RECORD_POST,profile+GRKIT_GRF_POST,GRKIT_GRF_MATRIX-GRKIT_GRF_POST);
    for(u32 i=0;i<9;++i) {
        int value=(features&GRKIT_FEATURE_FN_DEFAULTS) ? (signed char)profile[GRKIT_GRF_FN_DEFAULTS+i] : 0;
        out[GRKIT_RECORD_FN_DEFAULTS+i]=(u8)(value<0 ? value+9 : value);
    }
    for(u32 i=0;i<16 && profile[GRKIT_GRF_NAME+i];++i)
        out[GRKIT_RECORD_NAME+2*i]=profile[GRKIT_GRF_NAME+i];
    effect_record_copy(out+GRKIT_RECORD_ID,profile+GRKIT_GRF_ID,GRKIT_GRF_ID_SIZE);
    effect_record_store(out+GRKIT_RECORD_MONOCHROME,effect_record_word(profile+GRKIT_GRF_FLAGS));
    effect_record_store(out+GRKIT_RECORD_ICON,icon);
    out[GRKIT_RECORD_CODE]=(u8)code;
    out[GRKIT_RECORD_CODE+1]=(u8)(code>>8);
}
/* Unknown feature bits never enter the runtime builder. The owner also
   validates the complete GRF before requesting an App reservation. */
static u32 effect_record_hot_supported(const u8 *profile) {
    return !(effect_record_word(profile+GRKIT_GRF_FEATURES)&
             ~(GRKIT_FEATURE_POST|GRKIT_FEATURE_SELECTIVE|GRKIT_FEATURE_HUE|
               GRKIT_FEATURE_HUE_LEVELS|GRKIT_FEATURE_NATIVE_LOOK|GRKIT_FEATURE_FN_DEFAULTS));
}
#endif
