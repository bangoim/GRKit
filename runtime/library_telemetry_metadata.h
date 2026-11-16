#ifndef GRKIT_LIBRARY_TELEMETRY_METADATA_H
#define GRKIT_LIBRARY_TELEMETRY_METADATA_H
#include "library_snapshot_json.h"
#include "look_telemetry.h"
#define LIBRARY_TELEMETRY_INVALID 0U
#define LIBRARY_TELEMETRY_OK 1U
#define LIBRARY_TELEMETRY_NEEDS_RECONCILE 2U
struct library_telemetry_metadata {
    struct library_dependencies dependencies;
    u32 loaded[40],reserved_codes[40];
};
static int library_telemetry_overlap(const void *a,u32 an,const void *b,u32 bn)
{
    unsigned long x=(unsigned long)a,y=(unsigned long)b;
    return an && bn && (x<=y ? y-x<an : x-y<bn);
}
static int library_telemetry_entry(const struct library_manifest *manifest,const u8 id[32])
{
    for(u32 i=0;i<manifest->entries;++i)
        if(library_store_equal(library_manifest_record(manifest,i),id,32)) return (int)i;
    return -1;
}
/* Inputs must be immutable, validated owner snapshots. Scratch must not alias
   either input or metadata, and remains pinned with the resulting metadata.
   Failure never publishes metadata pointers or changes the source manifest. */
static u32 library_snapshot_from_telemetry(const struct library_manifest *manifest,
    const struct look_telemetry *telemetry,struct library_telemetry_metadata *scratch,
    struct library_snapshot_metadata *meta)
{
    if(!manifest || !manifest->bytes || !telemetry || !scratch || !meta ||
       manifest->entries>256 || manifest->listed>64 || !telemetry->revision ||
       (telemetry->seq&1U) || telemetry->flags || telemetry->count!=manifest->listed ||
       telemetry->record_count>40) return LIBRARY_TELEMETRY_INVALID;
    u32 length=32+96*manifest->entries+2*manifest->listed;
    if(library_telemetry_overlap(scratch,sizeof(*scratch),manifest->bytes,length) ||
       library_telemetry_overlap(scratch,sizeof(*scratch),manifest,sizeof(*manifest)) ||
       library_telemetry_overlap(scratch,sizeof(*scratch),telemetry,sizeof(*telemetry)) ||
       library_telemetry_overlap(scratch,sizeof(*scratch),meta,sizeof(*meta)) ||
       library_telemetry_overlap(meta,sizeof(*meta),manifest->bytes,length) ||
       library_telemetry_overlap(meta,sizeof(*meta),manifest,sizeof(*manifest)) ||
       library_telemetry_overlap(meta,sizeof(*meta),telemetry,sizeof(*telemetry))) return LIBRARY_TELEMETRY_INVALID;
    u32 loaded=0,reserved=0,selected=256,reconcile=0,recipes[2]={0},dial=0;
    scratch->dependencies.count=0;
    for(u32 i=0;i<GRKIT_RECIPE_SLOTS;++i) {
        u32 id=telemetry->recipe_ids[i];
        if(id>GRKIT_RECIPE_ID_MAX) return LIBRARY_TELEMETRY_INVALID;
        if(id) for(u32 j=0;j<i;++j)
            if(telemetry->recipe_ids[j]==id) return LIBRARY_TELEMETRY_INVALID;
    }
    for(u32 i=0;i<telemetry->record_count;++i) {
        const struct look_telemetry_record *record=telemetry->records+i;
        if(record->code>65535 || !(record->code&0x8000U) ||
           !library_manifest_text(record->id,32,31,1)) return LIBRARY_TELEMETRY_INVALID;
        for(u32 j=0;j<i;++j)
            if(record->code==telemetry->records[j].code ||
               library_dependency_code(record->id)==library_dependency_code(telemetry->records[j].id) ||
               library_store_equal(record->id,telemetry->records[j].id,32)) return LIBRARY_TELEMETRY_INVALID;
        int index=library_telemetry_entry(manifest,record->id);
        /* Known tombstones already reserve their identity in the manifest.
           Reserving their code again would reject restoring that same ID. */
        if(index<0)
            scratch->reserved_codes[reserved++]=library_dependency_code(record->id);
        else if(!(library_store_word(library_manifest_record(manifest,(u32)index)+84)&2U) &&
                !library_store_equal(record->hash,library_manifest_record(manifest,(u32)index)+52,32))
            return LIBRARY_TELEMETRY_INVALID;
    }
    for(u32 i=0;i<telemetry->count;++i) {
        const struct look_telemetry_entry *entry=telemetry->entries+i;
        int index=library_telemetry_entry(manifest,entry->id);
        if(index<0 || entry->enabled>1 || entry->loaded>1 || entry->position>=telemetry->count ||
           (entry->dial>>GRKIT_DIAL_USER_MODES)) return LIBRARY_TELEMETRY_INVALID;
        u32 flags=library_store_word(library_manifest_record(manifest,(u32)index)+84);
        if(flags&2U) return LIBRARY_TELEMETRY_INVALID;
        u32 code=library_dependency_code(entry->id),found=0;
        for(u32 j=0;j<i;++j)
            if(library_store_equal(entry->id,telemetry->entries[j].id,32) ||
               entry->position==telemetry->entries[j].position ||
               code==library_dependency_code(telemetry->entries[j].id)) return LIBRARY_TELEMETRY_INVALID;
        for(u32 j=0;j<reserved;++j)
            if(code==scratch->reserved_codes[j]) return LIBRARY_TELEMETRY_INVALID;
        for(u32 j=0;j<telemetry->record_count;++j)
            if(library_store_equal(entry->id,telemetry->records[j].id,32)) found=1;
        if(found!=entry->loaded || (entry->enabled && !entry->loaded)) return LIBRARY_TELEMETRY_INVALID;
        if(entry->loaded) {
            if(loaded==40) return LIBRARY_TELEMETRY_INVALID;
            scratch->loaded[loaded++]=(u32)index;
        }
        if(library_store_equal(entry->id,telemetry->selected_id,32)) {
            if(!entry->loaded || !telemetry->selected) return LIBRARY_TELEMETRY_INVALID;
            selected=(u32)index;
        }
        if((flags&1U)!=entry->enabled || library_manifest_order(manifest,entry->position)!=(u32)index) reconcile=1;
        for(u32 word=0;word<2;++word) {
            if(recipes[word]&entry->recipes[word]) return LIBRARY_TELEMETRY_INVALID;
            recipes[word]|=entry->recipes[word];
        }
        if(dial&entry->dial) return LIBRARY_TELEMETRY_INVALID;
        dial|=entry->dial;
        for(u32 j=0;j<GRKIT_RECIPE_SLOTS;++j) if(entry->recipes[j>>5]&(1U<<(j&31))) {
            if(!telemetry->recipe_ids[j] || scratch->dependencies.count==LIBRARY_DEPENDENCY_MAX) return LIBRARY_TELEMETRY_INVALID;
            scratch->dependencies.items[scratch->dependencies.count++]=(struct library_dependency){(u32)index,LIBRARY_DEPENDENCY_RECIPE,telemetry->recipe_ids[j]};
        }
        for(u32 j=0;j<GRKIT_DIAL_USER_MODES;++j) if(entry->dial&(1U<<j)) {
            if(scratch->dependencies.count==LIBRARY_DEPENDENCY_MAX) return LIBRARY_TELEMETRY_INVALID;
            scratch->dependencies.items[scratch->dependencies.count++]=(struct library_dependency){(u32)index,LIBRARY_DEPENDENCY_DIAL,j+1};
        }
    }
    if(selected==256 && telemetry->selected) return LIBRARY_TELEMETRY_INVALID;
    if(!telemetry->selected) for(u32 i=0;i<32;++i)
        if(telemetry->selected_id[i]) return LIBRARY_TELEMETRY_INVALID;
    if(reconcile) return LIBRARY_TELEMETRY_NEEDS_RECONCILE;
    meta->manifest=manifest; meta->loaded=scratch->loaded; meta->loaded_count=loaded;
    meta->selected=selected; meta->dependencies=&scratch->dependencies; meta->dependencies_complete=1;
    meta->reserved_codes=scratch->reserved_codes; meta->reserved_count=reserved;
    meta->blocked=0; meta->blocked_count=0;
    return LIBRARY_TELEMETRY_OK;
}
/* Only call after NEEDS_RECONCILE with the same pinned inputs. The separate
   output is a candidate, never an acknowledged runtime/storage generation. */
static int library_telemetry_reconcile(const struct library_manifest *manifest,
    const struct look_telemetry *telemetry,u8 *candidate,u32 capacity)
{
    if(!manifest || !manifest->bytes || !telemetry || !candidate || manifest->entries>256 || manifest->listed>64 ||
       telemetry->count!=manifest->listed) return 0;
    u32 length=32+96*manifest->entries+2*manifest->listed;
    if(length>capacity || library_telemetry_overlap(candidate,length,manifest->bytes,length) ||
       library_telemetry_overlap(candidate,length,manifest,sizeof(*manifest)) ||
       library_telemetry_overlap(candidate,length,telemetry,sizeof(*telemetry))) return 0;
    for(u32 i=0;i<length;++i) candidate[i]=manifest->bytes[i];
    for(u32 i=0;i<telemetry->count;++i) {
        const struct look_telemetry_entry *entry=telemetry->entries+i;
        int index=library_telemetry_entry(manifest,entry->id);
        if(index<0 || entry->position>=manifest->listed || entry->enabled>1) return 0;
        u8 *record=candidate+32+96*(u32)index;
        library_store_put(record+84,(library_store_word(record+84)&~1U)|entry->enabled);
        u8 *order=candidate+32+96*manifest->entries+2*entry->position;
        order[0]=(u8)index; order[1]=(u8)((u32)index>>8);
    }
    library_store_put(candidate+24,library_store_crc(candidate,length,24,4));
    struct library_manifest checked;
    return library_manifest_open(candidate,length,&checked);
}
#endif
