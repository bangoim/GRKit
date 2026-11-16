#ifndef GRKIT_PERSIST_VIEW_H
#define GRKIT_PERSIST_VIEW_H
#include "record_registry.h"
/* The complete value view of STATE.TXT. INSTALL captures the restored
   baseline; the App task captures later requests. Accessors select either
   live settings or the shared settings kept while a user-mode set applies.
   The consumer owns the buffer from READY until RELEASE, so capture may
   compare and replace words in place without another full-size buffer. */
static u32 persist_live_core(volatile u32 *h,u32 k) {
    static const u16 words[]={GRKIT_CORE_SETTING_WORDS};
    if(GRKIT_CORE_SETTING_BASE+k==GRKIT_CORE_TONE) {
        u32 active=h[GRKIT_HOST_ACTIVE/4];
        u32 i, n=h[GRKIT_HOST_COUNT/4];
        if(n>GRKIT_TEXT_RECORDS) return 0;
        for(i=0;i<n;++i)
            if(active==record_address(h,i)) return PERSIST_HALF(active,GRKIT_RECORD_CODE);
        return 0;
    }
    return h[words[k]/4];
}
static u32 persist_live_fn(u32 record) {
    u32 value=0, d=9, digit;
    while(d--) {
        digit=PERSIST_BYTE(record,GRKIT_RECORD_PARAMETERS+d);
        value=value*9U+(digit<=8U ? digit : 0U);
    }
    return value;
}
#ifndef PERSIST_CORE_VALUE
#define PERSIST_CORE_VALUE(h,k) persist_live_core(h,k)
#endif
#ifndef PERSIST_FN_VALUE
#define PERSIST_FN_VALUE(h,r,i) persist_live_fn(r)
#endif
#ifndef PERSIST_TONING_VALUE
#define PERSIST_TONING_VALUE(h,r,i) PERSIST_BYTE(r,GRKIT_RECORD_TONING)
#endif
#ifndef PERSIST_MODULE_VALUE
#define PERSIST_MODULE_VALUE(h,t,i) ((t)[SETTINGS_ENTRY(i)+1])
#endif
static u32 persist_put(volatile u32 *out,u32 offset,u32 value) {
    u32 changed=out[offset/4]!=value;
    out[offset/4]=value;
    return changed;
}
/* All unused words are zero. Revision follows effective content, not UI
   counters: temporary edits and recipe changes cannot postpone this file. */
#ifndef PERSIST_ID_BYTE
#define PERSIST_ID_BYTE(h,row,i) PERSIST_BYTE((h)[GRKIT_HOST_SERVICES/4],SERVICES_LOOK_IDS+(row)*32U+(i))
#endif
static u32 persist_parameter_id(volatile u32 *h,u32 list,u32 row,u8 id[32]) {
    u32 index=list ? PERSIST_WORD(list+EFFECTS_HEADER+row*EFFECTS_STRIDE,EFFECTS_RECORD) : row;
    u32 address=index<h[GRKIT_HOST_COUNT/4] ? record_address(h,index)+GRKIT_RECORD_ID : 0;
    for(u32 i=0;i<32;++i) id[i]=address ? PERSIST_BYTE(address,i) : PERSIST_ID_BYTE(h,row,i);
    return look_parameter_code(id);
}
static u32 persist_capture(volatile u32 *h,volatile u32 *out) {
    volatile u32 *t=PERSIST_SETTINGS(h);
    u32 changed=0, i, record, value, n=h[GRKIT_HOST_COUNT/4];
    for(i=0;i<GRKIT_PERSIST_CORE_COUNT;++i)
        changed|=persist_put(out,GRKIT_PERSIST_CORE+4U*i,PERSIST_CORE_VALUE(h,i));
    if(n>GRKIT_TEXT_RECORDS) n=0;
    u32 list=h[GRKIT_HOST_EFFECTS/4];
    u32 listed=list ? PERSIST_WORD(list,EFFECTS_COUNT) : 0;
    if(listed>GRKIT_PERSIST_EFFECT_CAPACITY) listed=0;
    if(list) n=listed;
    u32 written=0;
    for(i=0;i<n;++i) {
        u32 offset;
        u32 index=i, entry=list+EFFECTS_HEADER+i*EFFECTS_STRIDE;
        if(listed) index=PERSIST_WORD(entry,EFFECTS_RECORD);
        record=index<GRKIT_TEXT_RECORDS ? record_address(h,index) : 0;
        u8 id[32],other[32];
        u32 code=persist_parameter_id(h,list,i,id),fn=0,toning=0;
        if(!id[0] || (listed && PERSIST_WORD(entry,EFFECTS_PARAMETER_CODE)>0x7fffU)) continue;
        for(u32 j=0;j<n;++j) if(j!=i && persist_parameter_id(h,list,j,other)==code) { code=0xffffffffU; break; }
        if(code>0x7fffU) continue;
        if(record) {
            fn=PERSIST_FN_VALUE(h,record,index);
            toning=PERSIST_TONING_VALUE(h,record,index);
        } else if(listed) {
            fn=PERSIST_WORD(entry,EFFECTS_PARAMETER_FN);
            toning=PERSIST_WORD(entry,EFFECTS_PARAMETER_TONING);
        }
        if(code>0x7fffU) continue;
        offset=GRKIT_PERSIST_EFFECTS+4U*GRKIT_PERSIST_EFFECT_WORDS*written++;
        changed|=persist_put(out,offset,code);
        changed|=persist_put(out,offset+4U,fn);
        changed|=persist_put(out,offset+8U,toning<=8U ? toning : 0);
        for(u32 k=0;k<8;++k) changed|=persist_put(out,offset+12U+4U*k,look_parameter_id_word(id,k));
    }
    changed|=persist_put(out,GRKIT_PERSIST_EFFECT_COUNT,written);
    for(i=written;i<GRKIT_PERSIST_EFFECT_CAPACITY;++i)
        for(u32 k=0;k<GRKIT_PERSIST_EFFECT_WORDS;++k)
            changed|=persist_put(out,GRKIT_PERSIST_EFFECTS+4U*(i*GRKIT_PERSIST_EFFECT_WORDS+k),0);
    n=t ? t[0] : 0;
    if(n>GRKIT_MODULE_SETTINGS_MAX) n=0;
    changed|=persist_put(out,GRKIT_PERSIST_MODULE_COUNT,n);
    for(i=0;i<GRKIT_MODULE_SETTINGS_MAX;++i) {
        u32 offset=GRKIT_PERSIST_MODULES+4U*GRKIT_PERSIST_MODULE_WORDS*i;
        value=i<n ? t[SETTINGS_ENTRY(i)] : 0;
        changed|=persist_put(out,offset,value);
        changed|=persist_put(out,offset+4U,value ? PERSIST_MODULE_VALUE(h,t,i) : 0);
    }
    return changed;
}
static __attribute__((unused)) void persist_baseline(volatile u32 *h,volatile u32 *out) {
    persist_capture(h,out);
    out[GRKIT_PERSIST_REVISION/4]=1;
    out[GRKIT_PERSIST_REQUEST/4]=0;
    out[GRKIT_PERSIST_READY/4]=0;
    out[GRKIT_PERSIST_RELEASE/4]=0;
}
#endif
