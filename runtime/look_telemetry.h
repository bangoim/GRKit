#ifndef GRKIT_LOOK_TELEMETRY_H
#define GRKIT_LOOK_TELEMETRY_H
#include "abi.h"
struct look_telemetry_entry {
    u8 id[32];
    u32 enabled, position, recipes[2], dial, loaded;
};
#define LOOK_TELEMETRY_RECORD_HASHES 1
struct look_telemetry_record { u8 id[32]; u32 code; u8 hash[32]; };
struct look_telemetry {
    u32 seq, revision, count, state_changes, recipe_changes, dial_changes, selected, flags;
    u8 selected_id[32];
    struct look_telemetry_entry entries[64];
    u32 recipe_ids[GRKIT_RECIPE_SLOTS];
    u32 record_count;
    struct look_telemetry_record records[GRKIT_TEXT_RECORDS];
};
_Static_assert(sizeof(struct look_telemetry)==6628,"look telemetry services budget");
#ifndef LOOK_TELEMETRY_BARRIER
#define LOOK_TELEMETRY_BARRIER() __asm__ volatile("dmb ish" ::: "memory")
#endif
/* The owner accepts only a complete App snapshot. Zero revision means the
   first App tick has not captured it yet. No filesystem access occurs here. */
static u32 look_telemetry_copy(volatile const struct look_telemetry *from,struct look_telemetry *to) {
    u32 seq=from->seq;
    if(seq&1U) return 0;
    LOOK_TELEMETRY_BARRIER();
    for(u32 i=0;i<sizeof(*to);++i) ((u8 *)to)[i]=((volatile const u8 *)from)[i];
    LOOK_TELEMETRY_BARRIER();
    return seq==from->seq && to->revision && !to->flags && !from->flags;
}
#ifdef LOOK_TELEMETRY_RUNTIME
static __attribute__((noinline)) void look_telemetry_capture(volatile u32 *h) {
    volatile struct look_telemetry *t=(volatile struct look_telemetry *)services_word(h,SERVICES_LOOK_TELEMETRY);
    volatile const u8 *ids=(volatile const u8 *)services_word(h,SERVICES_LOOK_IDS);
    u32 n=effects_count(h),selected=h[GRKIT_HOST_CAPABILITIES/4]&GRKIT_CORE_TONE_BIT ? h[GRKIT_HOST_ACTIVE/4] : 0;
    u32 state=h[GRKIT_HOST_STATE_CHANGES/4],recipes=h[GRKIT_HOST_RECIPE_CHANGES/4],dial=h[GRKIT_HOST_DIAL_SET_CHANGES/4];
    u32 changed=0,seq=t->seq;
    u8 order[EFFECTS_LISTED];
    if(t->revision && t->state_changes==state && t->recipe_changes==recipes &&
       t->dial_changes==dial && t->selected==selected && t->count==n &&
       t->record_count==h[GRKIT_HOST_COUNT/4]) return;
    if(n>EFFECTS_LISTED || h[GRKIT_HOST_COUNT/4]>GRKIT_TEXT_RECORDS || seq>=0xfffffffcU || t->revision==0xffffffffU) {
        t->flags=1;
        return;
    }
    effects_order(h,order,n);
    t->seq=seq+1;
    LOOK_TELEMETRY_BARRIER();
    changed=t->count!=n;
    for(u32 i=0;i<32;++i) {
        u8 value=record_index(h,selected)<GRKIT_TEXT_RECORDS ? BYTE(selected,GRKIT_RECORD_ID+i) : 0;
        if(t->selected_id[i]!=value) changed=1;
        t->selected_id[i]=value;
    }
    for(u32 row=0;row<n;++row) {
        struct look_telemetry_entry entry;
        for(u32 i=0;i<sizeof(entry);++i) ((volatile u8 *)&entry)[i]=0;
        u32 hash=0x811c9dc5U,code;
        for(u32 i=0;i<32;++i) entry.id[i]=ids[row*32+i];
        for(u32 i=0;i<32 && entry.id[i];++i) hash=(hash^entry.id[i])*0x01000193U;
        code=hash&GRKIT_RECIPE_CODE_MASK;
        entry.enabled=!effects_off(h,row);
        entry.loaded=effects_record(h,row)!=EFFECTS_NO_RECORD;
        for(u32 i=0;i<n;++i) if(order[i]==row) entry.position=i;
        for(u32 i=0;i<GRKIT_RECIPE_SLOTS;++i) {
            u32 word=h[GRKIT_HOST_RECIPES/4+i*GRKIT_RECIPE_WORDS];
            if((word&RECIPE_USED) && RECIPE_LOOK(word)==RECIPE_LOOK_EFFECT && RECIPE_CODE(word)==code)
                entry.recipes[i>>5]|=1U<<(i&31);
        }
        for(u32 i=1;i<=GRKIT_DIAL_USER_MODES;++i)
            if(dial_has_set(h,i) && dial_set(h,i)[GRKIT_DIAL_SET_CORE+CORE_SETTING(GRKIT_CORE_TONE)]==(GRKIT_DIAL_SET_EFFECT|code))
                entry.dial|=1U<<(i-1);
        for(u32 i=0;i<sizeof(entry);++i) {
            u8 value=((u8 *)&entry)[i];
            if(((volatile u8 *)&t->entries[row])[i]!=value) changed=1;
            ((volatile u8 *)&t->entries[row])[i]=value;
        }
    }
    for(u32 i=0;i<GRKIT_RECIPE_SLOTS;++i) {
        u32 id=h[GRKIT_HOST_RECIPE_IDS/4+i];
        if(t->recipe_ids[i]!=id) changed=1;
        t->recipe_ids[i]=id;
    }
    {
        u32 count=h[GRKIT_HOST_COUNT/4];
        if(t->record_count!=count) changed=1;
        for(u32 i=0;i<count;++i) {
            u32 record=record_address(h,i),code=HALF(record,GRKIT_RECORD_CODE);
            for(u32 k=0;k<32;++k) {
                u8 value=BYTE(record,GRKIT_RECORD_ID+k);
                if(t->records[i].id[k]!=value) changed=1;
                t->records[i].id[k]=value;
                value=((volatile const u8 *)services_word(h,SERVICES_LOOK_HASHES))[i*32U+k];
                if(t->records[i].hash[k]!=value) changed=1;
                t->records[i].hash[k]=value;
            }
            if(t->records[i].code!=code) changed=1;
            t->records[i].code=code;
        }
        t->record_count=count;
    }
    if(changed || !t->revision) ++t->revision;
    t->count=n; t->state_changes=state; t->recipe_changes=recipes;
    t->dial_changes=dial; t->selected=selected; t->flags=0;
    LOOK_TELEMETRY_BARRIER();
    t->seq=seq+2;
}
#endif
#endif
