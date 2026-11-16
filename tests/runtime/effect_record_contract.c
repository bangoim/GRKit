#include <assert.h>
#include <stdint.h>
#include <string.h>
typedef uint32_t u32;
typedef uint8_t u8;
typedef uint16_t u16;
#include "effect_record.h"
#include "effect_multiaxial.h"

int main(void) {
    u8 profile[GRKIT_GRF_MAX_SIZE]={0},stock[GRKIT_RECORD_COEFFICIENTS],record[GRKIT_RECORD_SIZE];
    memset(stock,0x5a,sizeof(stock));
    memset(record,0xa5,sizeof(record));
    memcpy(profile+GRKIT_GRF_NAME,"Imported LUT",12);
    memcpy(profile+GRKIT_GRF_ID,"custom-lut",10);
    profile[GRKIT_GRF_MATRIX]=0x33;
    profile[GRKIT_GRF_CURVES+17]=0x44;
    effect_record_store(profile+GRKIT_GRF_FEATURES,GRKIT_FEATURE_POST|GRKIT_FEATURE_FN_DEFAULTS);
    profile[GRKIT_GRF_FN_DEFAULTS]=0xfc;
    profile[GRKIT_GRF_FN_DEFAULTS+1]=4;
    effect_record_prepare(record,stock,profile,0x59000000,0x59010000,0x8123);
    assert(effect_record_word(record)==0x5a5a5a5a);
    assert(effect_record_word(record+4)==0x59000000+GRKIT_RECORD_COEFFICIENTS);
    assert(record[GRKIT_RECORD_COEFFICIENTS]==0x33);
    assert(record[GRKIT_RECORD_CURVES+17]==0x44);
    assert(effect_record_word(record+GRKIT_RECORD_ICON)==0x59010000);
    assert(record[GRKIT_RECORD_CODE]==0x23 && record[GRKIT_RECORD_CODE+1]==0x81);
    assert(record[GRKIT_RECORD_FN_DEFAULTS]==5 && record[GRKIT_RECORD_FN_DEFAULTS+1]==4);
    for(u32 i=0;i<9;++i) assert(!record[GRKIT_RECORD_PARAMETERS+i]);
    for(u32 i=0;i<12;++i) {
        assert(record[GRKIT_RECORD_NAME+2*i]==profile[GRKIT_GRF_NAME+i]);
        assert(!record[GRKIT_RECORD_NAME+2*i+1]);
    }
    assert(!record[GRKIT_RECORD_NAME+24]);
    assert(!memcmp(record+GRKIT_RECORD_ID,"custom-lut",10));
    assert(effect_record_hot_supported(profile));
    effect_record_store(profile+GRKIT_GRF_FEATURES,GRKIT_FEATURE_NATIVE_LOOK);
    assert(effect_record_hot_supported(profile));
    effect_record_store(profile+GRKIT_GRF_FEATURES,GRKIT_FEATURE_HUE);
    assert(effect_record_hot_supported(profile));
    effect_record_store(profile+GRKIT_GRF_FEATURES,GRKIT_FEATURE_HUE);
    profile[GRKIT_GRF_HUE]=8; profile[GRKIT_GRF_HUE+1]=0xf8;
    effect_record_multiaxial(record,0x59000000,profile,0x53001234,0x53005678);
    u8 *multi=record+GRKIT_RECORD_MULTIAXIAL;
    assert(effect_record_word(multi+20)==0x53001234);
    assert(effect_record_word(multi+28)==0x53005678);
    assert(effect_record_word(multi+0x98)==1024);
    assert(effect_record_word(multi+0x98+5*36+4)==64);
    assert((int32_t)effect_record_word(multi+0x98+5*36+8)==-64);
    effect_record_store(profile+GRKIT_GRF_FEATURES,GRKIT_FEATURE_NATIVE_LOOK);
    for(u32 i=0;i<5;++i) profile[GRKIT_NATIVE_GAINS+i*2+1]=4;
    effect_record_multiaxial(record,0x59000000,profile,0x53001234,0x53005678);
    assert(effect_record_word(multi+4)==0x59000000+GRKIT_RECORD_MULTIAXIAL+8);
    assert(effect_record_word(multi+0x98)==1024);
    effect_record_store(profile+GRKIT_GRF_FEATURES,0x80000000U);
    assert(!effect_record_hot_supported(profile));
    return 0;
}
