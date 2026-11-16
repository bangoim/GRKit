#ifndef GRKIT_LIBRARY_PENDING_H
#define GRKIT_LIBRARY_PENDING_H
#include "library_candidate.h"
#include "look_telemetry.h"
/* A metadata PATCH may require one pending profile to be loaded. The App
   action publishes one record, so unrelated edits must remain separate. */
static int library_pending_index(const struct library_manifest *before,
    const struct library_manifest *after,const struct look_telemetry *telemetry,u32 *pending)
{
    if(!before || !after || !telemetry || !pending || telemetry->count>64) return 0;
    *pending=256;
    for(u32 i=0;i<after->entries;++i) {
        const u8 *entry=library_manifest_record(after,i);
        if(!(library_store_word(entry+84)&1U)) continue;
        u32 found=0;
        for(u32 row=0;row<telemetry->count;++row) {
            if(!library_store_equal(entry,telemetry->entries[row].id,32)) continue;
            if(++found!=1) return 0;
            if(!telemetry->entries[row].loaded) {
                if(*pending!=256) return 0;
                *pending=i;
            }
        }
        if(!found) return 0;
    }
    if(*pending==256) return 1;
    if(after->listed!=before->listed || after->entries!=before->entries) return 0;
    for(u32 i=0;i<before->entries;++i) {
        const u8 *old=library_manifest_record(before,i);
        int index=library_candidate_index(after,old);
        if(index<0 || ((u32)index!=*pending && !library_store_equal(old,library_manifest_record(after,(u32)index),96))) return 0;
    }
    for(u32 i=0;i<after->listed;++i)
        if(!library_store_equal(library_manifest_record(after,library_manifest_order(after,i)),
           library_manifest_record(before,library_manifest_order(before,i)),32)) return 0;
    return 1;
}
#endif
