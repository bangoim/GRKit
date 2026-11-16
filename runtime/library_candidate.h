#ifndef GRKIT_LIBRARY_CANDIDATE_H
#define GRKIT_LIBRARY_CANDIDATE_H
#include "library_content.h"
#define LIBRARY_CANDIDATE_ADD 0U
#define LIBRARY_CANDIDATE_METADATA 1U
#define LIBRARY_CANDIDATE_REPLACE 2U
/* Classify the bounded runtime action after the transport has validated both
   manifests and the optional GRF. Durable identity/tombstones cannot disappear.
   Official content is immutable; trusted Linux catalog resolution may promote
   an unchanged Unknown entry to Official, but never demote it. */
static int library_candidate_index(const struct library_manifest *manifest,const u8 id[32])
{
    for(u32 i=0;i<manifest->entries;++i)
        if(library_store_equal(library_manifest_record(manifest,i),id,32)) return (int)i;
    return -1;
}
static int library_candidate_classify(const struct library_manifest *before,const struct library_manifest *after,
    const u8 *content,u32 content_length,int (*validate)(const u8 *,u32),u32 *action,u32 *content_index)
{
    if(!before || !after || !validate || !action || !content_index || (content_length && !content)) return 0;
    u32 added=0,replaced=0,target=256;
    for(u32 i=0;i<before->entries;++i) {
        struct library_manifest_entry old,next;
        library_manifest_get(before,i,&old);
        int index=library_candidate_index(after,old.id);
        if(index<0 || !library_manifest_get(after,(u32)index,&next)) return 0;
        u32 old_origin=(old.flags>>3)&3U,next_origin=(next.flags>>3)&3U;
        int same_content=library_store_equal(old.hash,next.hash,32) &&
            library_store_equal(old.name,next.name,20) && old.features==next.features && !((old.flags^next.flags)&4U);
        if(old_origin!=next_origin && !(old_origin==0 && next_origin==2 && same_content)) return 0;
        if((old.flags&2U) && !(next.flags&2U)) {
            if(old_origin==0 || (old_origin==2 && !same_content) || !content_length ||
               !library_content_matches(&next,content,content_length,validate)) return 0;
            target=(u32)index; ++added; continue;
        }
        if(!same_content) {
            if(old_origin!=1 || next_origin!=1 || (old.flags&2U) || (next.flags&2U) ||
               !content_length || !library_content_matches(&next,content,content_length,validate)) return 0;
            target=(u32)index; ++replaced;
        }
    }
    for(u32 i=0;i<after->entries;++i) {
        struct library_manifest_entry next; library_manifest_get(after,i,&next);
        if(library_candidate_index(before,next.id)>=0) continue;
        if((next.flags&2U) || !((next.flags>>3)&3U) || !content_length ||
           !library_content_matches(&next,content,content_length,validate)) return 0;
        target=i; ++added;
    }
    if(added+replaced>1) return 0;
    if(added || replaced) {
        /* Add/replace publish one record. Other metadata changes would require
           a distinct App transaction, so reject a bundled multi-action edit. */
        for(u32 i=0;i<before->entries;++i) {
            const u8 *old=library_manifest_record(before,i);
            int index=library_candidate_index(after,old);
            if((u32)index==target) continue;
            const u8 *next=library_manifest_record(after,(u32)index);
            if(!library_store_equal(old,next,96)) return 0;
        }
        if(after->listed!=before->listed+added) return 0;
        for(u32 i=0;i<before->listed;++i) {
            const u8 *old=library_manifest_record(before,library_manifest_order(before,i));
            const u8 *next=library_manifest_record(after,library_manifest_order(after,i));
            if(!library_store_equal(old,next,32)) return 0;
        }
    }
    *action=added ? LIBRARY_CANDIDATE_ADD : replaced ? LIBRARY_CANDIDATE_REPLACE : LIBRARY_CANDIDATE_METADATA;
    *content_index=target; return 1;
}
#endif
