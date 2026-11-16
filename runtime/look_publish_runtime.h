#ifndef GRKIT_LOOK_PUBLISH_RUNTIME_H
#define GRKIT_LOOK_PUBLISH_RUNTIME_H
#include "effect_record.h"
#include "effect_multiaxial.h"
#include "look_publish.h"
#include "library_manifest.h"
#include "library_sha256.h"
#ifndef LOOK_PUBLISH_MAP
#define LOOK_PUBLISH_MAP(address) ((u8 *)(address))
#endif
#ifndef LOOK_PUBLISH_CURRENT_CONTROLLER
#define LOOK_PUBLISH_CURRENT_CONTROLLER(h) services_word(h,SERVICES_MENU+MENU_ROWS_CURRENT)[0]
#endif
/* Only the shooting screen has no native list that retains the old count.
   This is a UI lifetime check, not a claim that capture processing is idle. */
static u32 look_publish_ui_ready(volatile u32 *h) {
    return !h[GRKIT_HOST_MENU_CONTROLLER/4] && !h[GRKIT_HOST_PREVIEW/4] &&
           !h[GRKIT_HOST_DEVELOP/4] && !(h[GRKIT_HOST_PARAMETERS/4]&~1U) &&
           LOOK_PUBLISH_CURRENT_CONTROLLER(h)==SHOOTING_CONTROLLER;
}
#ifndef LOOK_PUBLISH_CAPTURE_IDLE
#ifdef FW_BE_ACTIVE_RECIPE_COUNT
#ifndef FW_BE_RELEASE_BUSY_OFFSET
#define FW_BE_RELEASE_BUSY_OFFSET 0xd6U
#define FW_BE_RECIPE_QUEUE_OFFSET 0x5acU
#endif
/* App serializes capture requests with publication. The front-end completion
   flag can precede back-end completion: require both the native request state
   and the same full recipe-list test used before AllImageWriteComplete. */
static u32 look_publish_capture_idle(void) {
    u32 model=CALL0(FW_SHOOTING_MODEL)(), process=CALL0(FW_BE_STILL_PROCESS)();
    if(!model || !process || CALL1(FW_CAPTURE_REQUEST_PENDING)(model) ||
       CALL1(FW_ALL_IMAGE_WRITES_COMPLETE)(model)!=1 || BYTE(process,FW_BE_RELEASE_BUSY_OFFSET)) return 0;
    u32 queue=WORD(process,FW_BE_RECIPE_QUEUE_OFFSET);
    if(!queue || CALL1(FW_BE_ACTIVE_RECIPE_COUNT)(queue)) return 0;
    return !BYTE(process,FW_BE_RELEASE_BUSY_OFFSET) && !CALL1(FW_CAPTURE_REQUEST_PENDING)(model) &&
           CALL1(FW_ALL_IMAGE_WRITES_COMPLETE)(model)==1;
}
#define LOOK_PUBLISH_CAPTURE_IDLE(h) look_publish_capture_idle()
#else
#define LOOK_PUBLISH_CAPTURE_IDLE(h) 0U
#endif
#endif
#include "look_metadata.h"
static u32 look_publish_key(const u8 *id) {
    const char *prefix="CORE.EFFECT.";
    u32 crc=~0U;
    for(u32 part=0;part<2;++part) {
        const u8 *p=part ? id : (const u8 *)prefix;
        for(;*p;++p) {
            crc^=*p;
            for(u32 bit=0;bit<8;++bit) crc=(crc>>1)^(0xedb88320U&(0U-(crc&1U)));
        }
    }
    crc=~crc;
    if(crc>=4000000000U) crc-=4000000000U;
    if(crc>=2000000000U) crc-=2000000000U;
    if(crc>=1000000000U) crc-=1000000000U;
    if(crc>=990000000U) crc-=10000000U;
    return crc ? crc : 1;
}
static u32 look_publish_grace(volatile u32 *h,volatile struct look_publish *p) {
    if(!h[GRKIT_HOST_TAIL/4]) return 0;
    if(!__atomic_load_n(&WORD(h[GRKIT_HOST_TAIL/4],GRKIT_GLOBAL_RECORD_READERS),__ATOMIC_SEQ_CST)) {
        p->retire_wait[0]=0; p->retire_wait[1]=0;
        return 1;
    }
    return 0;
}
static u32 look_publish_bank_address(volatile u32 *h,volatile const struct look_publish *p) {
    return (p->target_bank ? h[GRKIT_HOST_RECORD_BANK2/4] : h[GRKIT_HOST_RECORDS/4])+p->slot*GRKIT_RECORD_SIZE;
}
static __attribute__((noinline)) u32 look_publish_prepare_record(volatile u32 *h,volatile struct look_publish *p) {
    if(p->action==LOOK_PUBLISH_METADATA) return look_metadata_prepare(h,p);
    if(p->action!=LOOK_PUBLISH_ADD && p->action!=LOOK_PUBLISH_REPLACE) return LOOK_PUBLISH_UNSUPPORTED;
    u32 n=h[GRKIT_HOST_COUNT/4],rows=effects_count(h),hash=0x811c9dc5U,code,icon,pixels,header;
    u32 replacing=p->action==LOOK_PUBLISH_REPLACE,logical=n,row=rows;
    const u8 *profile=LOOK_PUBLISH_MAP(p->profile);
    if(!look_publish_ui_ready(h)) return LOOK_PUBLISH_BUSY;
    if(!h[GRKIT_HOST_EFFECTS/4] || !settings_table(h) || !p->profile ||
       (p->length!=GRKIT_GRF_SIZE && p->length!=GRKIT_GRF_MAX_SIZE) || p->enabled>1)
        return LOOK_PUBLISH_INVALID;
    if(!effect_record_hot_supported(profile) ||
       (NATIVE_MONOCHROME() && effect_record_word(profile+GRKIT_GRF_FLAGS)!=1U))
        return LOOK_PUBLISH_UNSUPPORTED;
    for(u32 i=0;i<rows;++i) {
        volatile const u8 *id=(volatile const u8 *)services_word(h,SERVICES_LOOK_IDS)+i*32;
        u32 equal=1;
        for(u32 k=0;k<32;++k) if(id[k]!=profile[GRKIT_GRF_ID+k]) equal=0;
        if(equal) { row=i; break; }
    }
    p->previous_slot=EFFECTS_NO_RECORD;
    p->evicted_row=EFFECTS_NO_RECORD;
    if(replacing) {
        if(row==rows) return LOOK_PUBLISH_CONFLICT;
        logical=effects_record(h,row);
        p->previous_slot=logical;
        if(logical==EFFECTS_NO_RECORD && !p->enabled) {
            for(u32 i=0;i<16;++i) if(p->name[i] && (p->name[i]<32 || p->name[i]>126)) return LOOK_PUBLISH_INVALID;
            if(!p->name[0] || p->name[16]) return LOOK_PUBLISH_INVALID;
            p->slot=EFFECTS_NO_RECORD; p->row=row; p->base_count=n;
            p->setting_slot=effects_handle(h,row)-effects_word(h,EFFECTS_SETTINGS)[0];
            return LOOK_PUBLISH_OK;
        }
        if(!h[GRKIT_HOST_RECORD_BANK2/4] || !h[GRKIT_HOST_TAIL/4]) return LOOK_PUBLISH_UNSUPPORTED;
    } else {
        if(row!=rows) return LOOK_PUBLISH_CONFLICT;
        if(rows>=EFFECTS_LISTED) return LOOK_PUBLISH_FULL;
    }
    if(!replacing || logical==EFFECTS_NO_RECORD) {
        logical=n;
        for(u32 i=0;i<n;++i) if(p->retired[i>>5]&(1U<<(i&31))) {
            if(logical==n) logical=i;
            u32 equal=1;
            for(u32 k=0;k<32;++k) if(BYTE(record_address(h,i),GRKIT_RECORD_ID+k)!=profile[GRKIT_GRF_ID+k]) equal=0;
            if(equal) { logical=i; break; }
        }
        if(logical>=GRKIT_TEXT_RECORDS) {
            for(u32 i=0;i<rows;++i) {
                u32 candidate=effects_record(h,i);
                if(i!=row && effects_off(h,i) && candidate!=EFFECTS_NO_RECORD) {
                    logical=candidate; p->evicted_row=i; break;
                }
            }
        }
        if(logical>=GRKIT_TEXT_RECORDS) return LOOK_PUBLISH_FULL;
    }
    look_publish_grace(h,p);
    if(p->retire_wait[logical>>5]&(1U<<(logical&31))) return LOOK_PUBLISH_BUSY;
    p->slot=logical; p->row=row; p->base_count=n;
    p->target_bank=logical<n ? 1U^((h[GRKIT_HOST_RECORD_BANK_BITS/4+(logical>>5)]>>(logical&31))&1U) : 0;
    if(p->target_bank && !h[GRKIT_HOST_RECORD_BANK2/4]) return LOOK_PUBLISH_UNSUPPORTED;
    for(u32 i=0;i<GRKIT_GRF_ID_SIZE && profile[GRKIT_GRF_ID+i];++i)
        hash=(hash^profile[GRKIT_GRF_ID+i])*0x01000193U;
    code=(replacing && p->previous_slot!=EFFECTS_NO_RECORD) ? HALF(record_address(h,logical),GRKIT_RECORD_CODE) : 0x8000U|(hash&0x7fffU);
    /* Reserve the GR IV native page across models so imported IDs travel. */
    if((code>>8)==0x81U) return LOOK_PUBLISH_CONFLICT;
    for(u32 i=0;i<n;++i) {
        if(i==logical || (p->retired[i>>5]&(1U<<(i&31)))) continue;
        if(HALF(record_address(h,i),GRKIT_RECORD_CODE)==code) return LOOK_PUBLISH_CONFLICT;
    }
    {
        volatile u32 *table=settings_table(h);
        u32 base=effects_word(h,EFFECTS_SETTINGS)[0],key=look_publish_key(profile+GRKIT_GRF_ID),free=EFFECTS_LISTED;
        if(replacing) free=effects_handle(h,row)-base;
        else for(u32 i=0;i<EFFECTS_LISTED && base+i<table[0];++i) {
            u32 used=0;
            for(u32 k=0;k<rows;++k) if(effects_handle(h,k)==base+i) used=1;
            if(!used) {
                if(free==EFFECTS_LISTED) free=i;
                if(table[SETTINGS_ENTRY(base+i)]==key) { free=i; break; }
            }
        }
        if(free==EFFECTS_LISTED) return LOOK_PUBLISH_FULL;
        p->setting_slot=free;
        for(u32 i=0;i<table[0];++i) if(i!=base+free && table[SETTINGS_ENTRY(i)]==key) return LOOK_PUBLISH_CONFLICT;
        for(u32 i=0;i<16;++i) if(p->name[i] && (p->name[i]<32 || p->name[i]>126)) return LOOK_PUBLISH_INVALID;
        if(!p->name[0] || p->name[16]) return LOOK_PUBLISH_INVALID;
    }
    {
        u32 features=effect_record_word(profile+GRKIT_GRF_FEATURES),tone_slot=0,needs_tone=0,used_tones=0;
        if((features&(GRKIT_FEATURE_HUE|GRKIT_FEATURE_NATIVE_LOOK)) &&
           (!p->multiaxial_regions || !p->multiaxial_rgb)) return LOOK_PUBLISH_UNSUPPORTED;
        if(features&GRKIT_FEATURE_NATIVE_LOOK) {
            for(u32 i=0;i<512;++i)
                if(profile[GRKIT_GRF_CURVES+i]!=profile[GRKIT_GRF_CURVES+512+i] ||
                   profile[GRKIT_GRF_CURVES+1024+i]!=profile[GRKIT_GRF_CURVES+512+i]) needs_tone=1;
        }
        if(needs_tone) {
            for(u32 i=0;i<n;++i) {
                if(i==logical || (p->retired[i>>5]&(1U<<(i&31)))) continue;
                u32 used=BYTE(record_address(h,i),GRKIT_RECORD_TONE_SLOT);
                if(used && used<=GRKIT_TONE_SLOTS) used_tones|=1U<<(used-1);
            }
            while(tone_slot<GRKIT_TONE_SLOTS && (used_tones&(1U<<tone_slot))) ++tone_slot;
            if(tone_slot>=GRKIT_TONE_SLOTS) return LOOK_PUBLISH_FULL;
        }
        u32 address=look_publish_bank_address(h,p);
        effect_record_prepare(LOOK_PUBLISH_MAP(address),(const u8 *)p->stock,profile,address,0,code);
        if(features&(GRKIT_FEATURE_HUE|GRKIT_FEATURE_NATIVE_LOOK))
            effect_record_multiaxial(LOOK_PUBLISH_MAP(address),address,profile,p->multiaxial_regions,p->multiaxial_rgb);
        if(replacing && p->previous_slot!=EFFECTS_NO_RECORD) for(u32 i=0;i<9;++i)
            BYTE(address,GRKIT_RECORD_PARAMETERS+i)=BYTE(record_address(h,logical),GRKIT_RECORD_PARAMETERS+i);
        if(replacing && p->previous_slot==EFFECTS_NO_RECORD) {
            u32 value=MANAGE_MAP(effects_entry(h,p->row)+EFFECTS_PARAMETER_FN)[0];
            recipe_fn_write(address,value);
        }
        if(needs_tone) {
            u32 curves=h[GRKIT_HOST_TAIL/4]+GRKIT_LOOK_PREPARED_TONE;
            effect_record_copy(LOOK_PUBLISH_MAP(curves),profile+GRKIT_GRF_CURVES,512);
            effect_record_copy(LOOK_PUBLISH_MAP(curves+512),profile+GRKIT_GRF_CURVES+1024,512);
            BYTE(address,GRKIT_RECORD_TONE_SLOT)=(u8)(tone_slot+1);
            if(replacing) {
                u32 value=p->previous_slot!=EFFECTS_NO_RECORD ? BYTE(record_address(h,logical),GRKIT_RECORD_TONING)
                    : MANAGE_MAP(effects_entry(h,p->row)+EFFECTS_PARAMETER_TONING)[0];
                BYTE(address,GRKIT_RECORD_TONING)=(u8)value;
                toning_curves(address+GRKIT_RECORD_CURVES,curves,toning_value(value));
            }
        }
    }
    {
        /* Volatile keeps this bounded reduction freestanding under -Oz. */
        volatile u32 slot=GRKIT_HOST_ICON_COUNT+logical+p->target_bank*GRKIT_TEXT_RECORDS;
        u32 chunk=0;
        while(slot>=GRKIT_ICONS_PER_CHUNK) { slot-=GRKIT_ICONS_PER_CHUNK; ++chunk; }
        icon=h[GRKIT_HOST_ICON_CHUNKS/4+chunk]+slot*GRKIT_ICON_STRIDE;
        if(!h[GRKIT_HOST_ICON_CHUNKS/4+chunk]) return LOOK_PUBLISH_FULL;
    }
    header=GRKIT_ICON_STRIDE-GRKIT_GRF_ICON_SIZE;
    if(header!=8 && header!=12) return LOOK_PUBLISH_UNSUPPORTED;
    pixels=h[GRKIT_HOST_OFF_ICON/4]+header-WORD(h[GRKIT_HOST_OFF_ICON/4],header-4)*4;
    if(header==12) { WORD(icon,0)=40U<<16|1U; WORD(icon,4)=40; }
    else WORD(icon,0)=40U<<16|40U;
    WORD(icon,header-4)=(icon+header-pixels)/4;
    effect_record_copy(LOOK_PUBLISH_MAP(icon+header),profile+GRKIT_GRF_ICON,GRKIT_GRF_ICON_SIZE);
    WORD(look_publish_bank_address(h,p),GRKIT_RECORD_ICON)=icon;
    if(!library_sha256(profile,p->length,(u8 *)p->pending_hash)) return LOOK_PUBLISH_INVALID;
    p->code=code;
    return LOOK_PUBLISH_OK;
}
static __attribute__((noinline)) u32 look_publish_commit_record(volatile u32 *h,volatile struct look_publish *p) {
    if(!LOOK_PUBLISH_CAPTURE_IDLE(h)) return LOOK_PUBLISH_BUSY;
    if(p->action==LOOK_PUBLISH_METADATA) return look_metadata_commit(h,p);
    if(p->action!=LOOK_PUBLISH_ADD && p->action!=LOOK_PUBLISH_REPLACE) return LOOK_PUBLISH_RECOVERY;
    u32 replacing=p->action==LOOK_PUBLISH_REPLACE;
    if(replacing && p->slot==EFFECTS_NO_RECORD) {
        if(!look_publish_ui_ready(h)) return LOOK_PUBLISH_BUSY;
        if(effects_record(h,p->row)!=EFFECTS_NO_RECORD || p->enabled) return LOOK_PUBLISH_RECOVERY;
        u32 entry=effects_entry(h,p->row);
        for(u32 i=0;i<EFFECTS_NAME_UNITS;++i)
            ((volatile u16 *)MANAGE_MAP(entry))[EFFECTS_NAME/2+i]=i<16 ? p->name[i] : 0;
        GRKIT_STATE_CHANGED(h);
        LOOK_PUBLISH_CAPTURE(h);
        return LOOK_PUBLISH_OK;
    }
    u32 record=look_publish_bank_address(h,p),old=record_address(h,p->slot),entry,handle,key;
    volatile u32 *table=settings_table(h);
    if(!look_publish_ui_ready(h)) return LOOK_PUBLISH_BUSY;
    if(!table || h[GRKIT_HOST_COUNT/4]!=p->base_count ||
       (!replacing && effects_count(h)!=p->row) || (replacing && effects_record(h,p->row)!=p->previous_slot))
        return LOOK_PUBLISH_RECOVERY;
    handle=effects_word(h,EFFECTS_SETTINGS)[0]+p->setting_slot;
    if(handle>=table[0]) return LOOK_PUBLISH_RECOVERY;
    key=look_publish_key(LOOK_PUBLISH_MAP(record+GRKIT_RECORD_ID));
    for(u32 i=0;i<table[0];++i) if(i!=handle && table[SETTINGS_ENTRY(i)]==key) return LOOK_PUBLISH_RECOVERY;
    entry=effects_entry(h,p->row);
    for(u32 i=0;i<EFFECTS_NAME_UNITS;++i)
        ((volatile u16 *)MANAGE_MAP(entry))[EFFECTS_NAME/2+i]=i<16 ? p->name[i] : 0;
    /* The manifest name may differ from the embedded GRF name. */
    for(u32 i=0;i<18;++i) HALF(record,GRKIT_RECORD_NAME+i*2)=i<16 ? p->name[i] : 0;
    for(u32 language=0;language<GRKIT_TEXT_LANGUAGES;++language) {
        u32 base=GRKIT_TEXT(h),out=base+GRKIT_TEXT_SUMMARY+(language*GRKIT_TEXT_RECORDS+p->slot)*GRKIT_SUMMARY_UNITS*2;
        const u16 *prefix=MENU_TEXT(base+MENU_TEXT(base)[language*GRKIT_TEXT_STRINGS+TEXT_IMAGE_PREFIX]);
        u32 k=0;
        while(prefix[k] && k<GRKIT_SUMMARY_UNITS-1) { ((volatile u16 *)MENU_TEXT(out))[k]=prefix[k]; ++k; }
        for(u32 j=0;j<16 && p->name[j] && k<GRKIT_SUMMARY_UNITS-1;++j) { ((volatile u16 *)MENU_TEXT(out))[k]=p->name[j]; ++k; }
        ((volatile u16 *)MENU_TEXT(out))[k]=0;
    }
    for(u32 i=0;i<32;++i)
        ((volatile u8 *)services_word(h,SERVICES_LOOK_IDS))[p->row*32+i]=BYTE(record,GRKIT_RECORD_ID+i);
    table[SETTINGS_ENTRY(handle)]=key;
    table[SETTINGS_ENTRY(handle)+1]=(replacing ? table[SETTINGS_ENTRY(handle)+1]&~1U : 0U)|!p->enabled;
    table[SETTINGS_ENTRY(handle)+2]=EFFECT_CHOICE_VALUES;
    MANAGE_MAP(entry)[EFFECTS_STATUS/4]=p->enabled ? EFFECT_LOADED : EFFECT_OFF;
    MANAGE_MAP(entry)[EFFECTS_RECORD/4]=p->slot;
    MANAGE_MAP(entry)[EFFECTS_SLOT/4]=p->setting_slot;
    if(p->evicted_row!=EFFECTS_NO_RECORD) {
        u32 evicted=effects_entry(h,p->evicted_row);
        u32 active=dial_word(h,DIAL_ACTIVE)[0];
        MANAGE_MAP(evicted)[EFFECTS_PARAMETER_CODE/4]=look_parameter_code((volatile u8 *)LOOK_PUBLISH_MAP(old+GRKIT_RECORD_ID));
        MANAGE_MAP(evicted)[EFFECTS_PARAMETER_FN/4]=active ? dial_word(h,DIAL_BASE_FN+4U*p->slot)[0] : recipe_fn_read(old);
        MANAGE_MAP(evicted)[EFFECTS_PARAMETER_TONING/4]=active ? ((volatile u8 *)dial_word(h,DIAL_BASE_TONING))[p->slot] : BYTE(old,GRKIT_RECORD_TONING);
        MANAGE_MAP(evicted)[EFFECTS_RECORD/4]=EFFECTS_NO_RECORD;
        MANAGE_MAP(evicted)[EFFECTS_STATUS/4]=EFFECT_OFF;
    }
    u32 tone=BYTE(record,GRKIT_RECORD_TONE_SLOT);
    if(tone) effect_record_copy(LOOK_PUBLISH_MAP(h[GRKIT_HOST_TAIL/4]+GRKIT_TONE_CURVES+(tone-1)*GRKIT_TONE_SLOT_SIZE),
                               LOOK_PUBLISH_MAP(h[GRKIT_HOST_TAIL/4]+GRKIT_LOOK_PREPARED_TONE),GRKIT_TONE_SLOT_SIZE);
    for(u32 i=0;i<32;++i) ((volatile u8 *)services_word(h,SERVICES_LOOK_HASHES+p->slot*32))[i]=((volatile u8 *)p->pending_hash)[i];
    if(!replacing) {
        MANAGE_MAP(entry)[EFFECTS_PARAMETER_CODE/4]=look_parameter_code((volatile u8 *)LOOK_PUBLISH_MAP(record+GRKIT_RECORD_ID));
        MANAGE_MAP(entry)[EFFECTS_PARAMETER_FN/4]=0;
        MANAGE_MAP(entry)[EFFECTS_PARAMETER_TONING/4]=0;
    }
    dial_publish_record(h,p->slot,record,replacing && p->previous_slot!=EFFECTS_NO_RECORD);
    if(!p->enabled) dial_forget_record(h,p->slot);
    LOOK_PUBLISH_BARRIER();
    u32 mask=1U<<(p->slot&31),word=GRKIT_HOST_RECORD_BANK_BITS/4+(p->slot>>5);
    h[word]=(h[word]&~mask)|(p->target_bank ? mask : 0);
    if(h[GRKIT_HOST_ACTIVE/4]==old) h[GRKIT_HOST_ACTIVE/4]=p->enabled ? record : 0;
    h[GRKIT_HOST_PARAMETERS/4]=0;
    h[GRKIT_HOST_DEVELOP_FILLED/4]=0;
    h[GRKIT_HOST_DEVELOP_CHANGED/4]=0;
    h[GRKIT_HOST_GAMMA_KEY/4]=0;
    h[GRKIT_HOST_LIVE_RECORD/4]=0;
    ++h[GRKIT_HOST_PARAMETER_CHANGES/4];
    if(p->slot==p->base_count) h[GRKIT_HOST_COUNT/4]=p->slot+1;
    if(!replacing) effects_word(h,EFFECTS_COUNT)[0]=p->row+1;
    p->retired[p->slot>>5]&=~mask;
    LOOK_PUBLISH_BARRIER();
    if(p->slot<p->base_count) p->retire_wait[p->slot>>5]|=mask;
    tone_stored(h);
    GRKIT_STATE_CHANGED(h);
    /* Native IconView retains its ID and otherwise skips same-ID redraws. */
    if(h[GRKIT_HOST_FRAME_CANVAS/4]) MENU_PREVIEW_INVALIDATE(h[GRKIT_HOST_FRAME_CANVAS/4]);
    /* The owner may resume as soon as done is published. Its snapshot must
       already describe the newly visible record at that boundary. */
    LOOK_PUBLISH_CAPTURE(h);
    return LOOK_PUBLISH_OK;
}
#endif
