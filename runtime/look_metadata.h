#ifndef GRKIT_LOOK_METADATA_H
#define GRKIT_LOOK_METADATA_H
/* The owner classifies content-preserving changes before requesting this
   path. The candidate stays immutable through PREPARE and COMMIT. */
static u32 look_metadata_entry(const struct library_manifest *manifest,const volatile u8 *id) {
    for(u32 i=0;i<manifest->entries;++i) {
        const u8 *entry=library_manifest_record(manifest,i);
        u32 equal=1;
        for(u32 k=0;k<32;++k) if(entry[k]!=id[k]) equal=0;
        if(equal && !(library_store_word(entry+84)&2U)) return i;
    }
    return 256;
}
static __attribute__((noinline)) u32 look_metadata_prepare(volatile u32 *h,volatile struct look_publish *p) {
    struct library_manifest manifest;
    volatile const u8 *ids=(volatile const u8 *)services_word(h,SERVICES_LOOK_IDS);
    u32 n=effects_count(h);
    if(!look_publish_ui_ready(h)) return LOOK_PUBLISH_BUSY;
    if(p->select_off>1 || !p->manifest || !library_manifest_open(LOOK_PUBLISH_MAP(p->manifest),p->manifest_length,&manifest))
        return LOOK_PUBLISH_INVALID;
    if(manifest.listed>n) return LOOK_PUBLISH_UNSUPPORTED;
    u32 found=0;
    for(u32 row=0;row<n;++row) {
        u32 index=look_metadata_entry(&manifest,ids+row*32);
        if(index==256) continue;
        const u8 *entry=library_manifest_record(&manifest,index);
        if((library_store_word(entry+84)&1U) && effects_record(h,row)==EFFECTS_NO_RECORD)
            return LOOK_PUBLISH_UNSUPPORTED;
        const u16 *name=effects_name(h,row);
        for(u32 k=0;k<17;++k) if(name[k]!=entry[32+k]) return LOOK_PUBLISH_UNSUPPORTED;
        ++found;
    }
    if(found!=manifest.listed) return LOOK_PUBLISH_CONFLICT;
    p->row=n;
    return LOOK_PUBLISH_OK;
}
static __attribute__((noinline)) u32 look_metadata_commit(volatile u32 *h,volatile struct look_publish *p) {
    struct library_manifest manifest;
    volatile u8 *ids=(volatile u8 *)services_word(h,SERVICES_LOOK_IDS);
    volatile u32 *table=settings_table(h);
    u32 n=effects_count(h),out=0,clear=p->select_off;
    if(!look_publish_ui_ready(h)) return LOOK_PUBLISH_BUSY;
    if(n!=p->row || !library_manifest_open(LOOK_PUBLISH_MAP(p->manifest),p->manifest_length,&manifest))
        return LOOK_PUBLISH_RECOVERY;
    for(u32 row=0;row<n;++row) {
        u32 index=look_metadata_entry(&manifest,ids+row*32),record=effects_record(h,row),handle=effects_handle(h,row);
        u32 enabled=0,position=0;
        if(index!=256) {
            enabled=library_store_word(library_manifest_record(&manifest,index)+84)&1U;
            for(u32 i=0;i<manifest.listed;++i) if(library_manifest_order(&manifest,i)==index) position=i+1;
        }
        /* Keep the key and slot allocated: retained capture readers and
           persisted settings never become another Look's identity. */
        table[SETTINGS_ENTRY(handle)+1]=(position<<1)|!enabled;
        if(!enabled && record!=EFFECTS_NO_RECORD) {
            dial_forget_record(h,record);
            if(h[GRKIT_HOST_ACTIVE/4]==tone_record(h,record+1)) clear=1;
        }
        if(index==256) {
            if(record!=EFFECTS_NO_RECORD) {
                p->retired[record>>5]|=1U<<(record&31);
                p->retire_wait[record>>5]|=1U<<(record&31);
            }
            continue;
        }
        u32 from=effects_entry(h,row),to=effects_entry(h,out);
        for(u32 i=0;i<EFFECTS_STRIDE/4;++i) MANAGE_MAP(to)[i]=MANAGE_MAP(from)[i];
        MANAGE_MAP(to)[EFFECTS_STATUS/4]=enabled ? EFFECT_LOADED : EFFECT_OFF;
        for(u32 i=0;i<32;++i) ids[out*32+i]=ids[row*32+i];
        ++out;
    }
    /* Removed rows cannot donate cached parameters to a future identity. */
    for(u32 row=out;row<n;++row) {
        u32 entry=effects_entry(h,row);
        MANAGE_MAP(entry)[EFFECTS_PARAMETER_CODE/4]=0;
        MANAGE_MAP(entry)[EFFECTS_PARAMETER_FN/4]=0;
        MANAGE_MAP(entry)[EFFECTS_PARAMETER_TONING/4]=0;
    }
    if(clear) {
        h[GRKIT_HOST_ACTIVE/4]=0;
        h[GRKIT_HOST_PREVIEW/4]=0;
        tone_stored(h);
    }
    LOOK_PUBLISH_BARRIER();
    effects_word(h,EFFECTS_COUNT)[0]=out;
    GRKIT_STATE_CHANGED(h);
    LOOK_PUBLISH_CAPTURE(h);
    return LOOK_PUBLISH_OK;
}
#endif
