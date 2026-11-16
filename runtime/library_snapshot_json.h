#ifndef GRKIT_LIBRARY_SNAPSHOT_JSON_H
#define GRKIT_LIBRARY_SNAPSHOT_JSON_H
#include "library_dependencies.h"
#define LIBRARY_SNAPSHOT_JSON_MAX (128U*1024U)
/* All inputs are pinned for the entire serialization/stream lifetime. The sink
   can hash, count, or copy a range without allocating the full metadata JSON. */
struct library_snapshot_official { const u8 *id,*hash; };
struct library_snapshot_metadata {
    const struct library_manifest *manifest;
    const char *card_id;
    u32 epoch[4],generation;
    const u32 *loaded; u32 loaded_count,selected;
    const struct library_dependencies *dependencies;
    u32 dependencies_complete;
    const u32 *blocked; u32 blocked_count;
    const u32 *reserved_codes; u32 reserved_count;
    u32 supported_features,monochrome_only;
    const struct library_snapshot_official *official; u32 official_count;
    u32 hot_reload;
};
struct library_json_sink {
    void *context;
    int (*write)(void *,const u8 *,u32);
    u32 count,failed;
};
static void library_json_bytes(struct library_json_sink *out,const u8 *bytes,u32 length)
{
    if(out->failed) return;
    if(length>LIBRARY_SNAPSHOT_JSON_MAX-out->count || !out->write(out->context,bytes,length)) { out->failed=1; return; }
    out->count+=length;
}
static void library_json_literal(struct library_json_sink *out,const char *text)
{
    u32 n=0; while(text[n]) ++n;
    library_json_bytes(out,(const u8 *)text,n);
}
static void library_json_text(struct library_json_sink *out,const u8 *text,u32 maximum)
{
    library_json_literal(out,"\"");
    for(u32 i=0;i<maximum && text[i];++i) {
        u8 c=text[i];
        if(c<32 || c>126) { out->failed=1; return; }
        if(c=='"' || c=='\\') library_json_literal(out,"\\");
        library_json_bytes(out,&c,1);
    }
    library_json_literal(out,"\"");
}
static void library_json_number(struct library_json_sink *out,u32 number)
{
    /* Division becomes a missing EABI helper in the size-optimized owner. */
    static const u32 places[10]={1000000000U,100000000U,10000000U,1000000U,
        100000U,10000U,1000U,100U,10U,1U};
    u32 emitted=0;
    for(u32 i=0;i<10;++i) {
        u8 digit='0';
        while(number>=places[i]) { number-=places[i]; ++digit; }
        if(digit!='0' || emitted || i==9) {
            library_json_bytes(out,&digit,1); emitted=1;
        }
    }
}
static void library_json_bool(struct library_json_sink *out,u32 value)
{ library_json_literal(out,value?"true":"false"); }
static void library_json_hash(struct library_json_sink *out,const u8 hash[32])
{
    static const char hex[]="0123456789abcdef";
    library_json_literal(out,"\"");
    for(u32 i=0;i<32;++i) {
        u8 pair[2]={(u8)hex[hash[i]>>4],(u8)hex[hash[i]&15U]}; library_json_bytes(out,pair,2);
    }
    library_json_literal(out,"\"");
}
static void library_json_ids(struct library_json_sink *out,const struct library_manifest *manifest,const u32 *indices,u32 count)
{
    library_json_literal(out,"[");
    for(u32 i=0;i<count;++i) {
        if(i) library_json_literal(out,",");
        if(indices[i]>=manifest->entries) { out->failed=1; return; }
        for(u32 j=0;j<i;++j) if(indices[j]==indices[i]) { out->failed=1; return; }
        library_json_text(out,library_manifest_record(manifest,indices[i]),31);
    }
    library_json_literal(out,"]");
}
static void library_json_manifest(struct library_json_sink *out,const struct library_manifest *manifest)
{
    library_json_literal(out,"{\"formatVersion\":1,\"entries\":{");
    for(u32 i=0;i<manifest->entries;++i) {
        struct library_manifest_entry entry; library_manifest_get(manifest,i,&entry);
        if(i) library_json_literal(out,",");
        library_json_text(out,entry.id,31); library_json_literal(out,":{\"name\":");
        library_json_text(out,entry.name,16); library_json_literal(out,",\"contentHash\":");
        library_json_hash(out,entry.hash); library_json_literal(out,",\"enabled\":");
        library_json_bool(out,entry.flags&1U); library_json_literal(out,",\"origin\":");
        library_json_literal(out,((entry.flags>>3)&3U)==2?"\"official\"":((entry.flags>>3)&3U)==1?"\"custom\"":"\"unknown\"");
        library_json_literal(out,",\"removed\":"); library_json_bool(out,entry.flags&2U);
        library_json_literal(out,",\"features\":"); library_json_number(out,entry.features);
        library_json_literal(out,",\"monochrome\":"); library_json_bool(out,entry.flags&4U);
        library_json_literal(out,"}");
    }
    library_json_literal(out,"},\"order\":[");
    for(u32 i=0;i<manifest->listed;++i) {
        if(i) library_json_literal(out,",");
        library_json_text(out,library_manifest_record(manifest,library_manifest_order(manifest,i)),31);
    }
    library_json_literal(out,"]}");
}
static void library_json_dependencies(struct library_json_sink *out,const struct library_snapshot_metadata *meta)
{
    library_json_literal(out,"{");
    u32 emitted=0;
    for(u32 entry=0;entry<meta->manifest->entries;++entry) {
        u32 found=0;
        for(u32 j=0;j<meta->dependencies->count;++j) if(meta->dependencies->items[j].entry==entry) ++found;
        if(!found) continue;
        if(emitted++) library_json_literal(out,",");
        library_json_text(out,library_manifest_record(meta->manifest,entry),31); library_json_literal(out,":[");
        u32 n=0;
        for(u32 j=0;j<meta->dependencies->count;++j) {
            const struct library_dependency *dep=meta->dependencies->items+j;
            if(dep->entry!=entry) continue;
            if(n++) library_json_literal(out,",");
            library_json_literal(out,dep->kind==LIBRARY_DEPENDENCY_RECIPE?"\"recipe:":"\"dialSet:");
            library_json_number(out,dep->id); library_json_literal(out,"\"");
        }
        library_json_literal(out,"]");
    }
    library_json_literal(out,"}");
}
static int library_snapshot_json(const struct library_snapshot_metadata *meta,struct library_json_sink *out)
{
    if(out) out->count=out->failed=0;
    if(!meta || !out || !out->write || !meta->manifest || !meta->manifest->bytes ||
       meta->manifest->entries>256 || meta->manifest->listed>64 || meta->manifest->loaded>40 ||
       !meta->card_id || !meta->dependencies || meta->hot_reload>1 ||
       meta->loaded_count>40 || meta->blocked_count>256 || meta->reserved_count>256 || meta->official_count>256 ||
       (meta->loaded_count && !meta->loaded) || (meta->blocked_count && !meta->blocked) ||
       (meta->reserved_count && !meta->reserved_codes) || (meta->official_count && !meta->official) ||
       meta->dependencies->count>LIBRARY_DEPENDENCY_MAX) return 0;
    for(u32 i=0;i<meta->loaded_count;++i) {
        if(meta->loaded[i]>=meta->manifest->entries || (library_store_word(library_manifest_record(meta->manifest,meta->loaded[i])+84)&2U)) return 0;
        for(u32 j=0;j<i;++j) if(meta->loaded[j]==meta->loaded[i]) return 0;
    }
    if(meta->selected!=256 && (meta->selected>=meta->manifest->entries ||
       (library_store_word(library_manifest_record(meta->manifest,meta->selected)+84)&2U))) return 0;
    for(u32 i=0;i<meta->manifest->entries;++i) {
        if(!(library_store_word(library_manifest_record(meta->manifest,i)+84)&1U)) continue;
        u32 found=0; for(u32 j=0;j<meta->loaded_count;++j) if(meta->loaded[j]==i) found=1;
        if(!found) return 0;
    }
    u32 card_length=0; while(card_length<=128 && meta->card_id[card_length]) ++card_length;
    if(!card_length || card_length>128 || !(meta->epoch[0]|meta->epoch[1]|meta->epoch[2]|meta->epoch[3])) return 0;
    for(u32 i=0;i<meta->dependencies->count;++i) {
        const struct library_dependency *dep=meta->dependencies->items+i;
        if(dep->entry>=meta->manifest->entries || !dep->id ||
           (dep->kind!=LIBRARY_DEPENDENCY_RECIPE && dep->kind!=LIBRARY_DEPENDENCY_DIAL)) return 0;
    }
    out->count=out->failed=0;
    library_json_literal(out,"{\"manifest\":"); library_json_manifest(out,meta->manifest);
    library_json_literal(out,",\"cardId\":"); library_json_text(out,(const u8 *)meta->card_id,128);
    library_json_literal(out,",\"runtimeEpoch\":[");
    for(u32 i=0;i<4;++i) { if(i) library_json_literal(out,","); library_json_number(out,meta->epoch[i]); }
    library_json_literal(out,"],\"generation\":"); library_json_number(out,meta->generation);
    library_json_literal(out,",\"loadedIds\":"); library_json_ids(out,meta->manifest,meta->loaded,meta->loaded_count);
    library_json_literal(out,",\"context\":{\"hotReload\":"); library_json_bool(out,meta->hot_reload);
    library_json_literal(out,",\"selected\":");
    if(meta->selected==256) library_json_literal(out,"null");
    else if(meta->selected<meta->manifest->entries) library_json_text(out,library_manifest_record(meta->manifest,meta->selected),31);
    else return 0;
    library_json_literal(out,",\"dependenciesComplete\":"); library_json_bool(out,meta->dependencies_complete);
    library_json_literal(out,",\"dependencies\":"); library_json_dependencies(out,meta);
    library_json_literal(out,",\"blockedIds\":"); library_json_ids(out,meta->manifest,meta->blocked,meta->blocked_count);
    library_json_literal(out,",\"reservedCodes\":[");
    for(u32 i=0;i<meta->reserved_count;++i) {
        if(meta->reserved_codes[i]>65535) return 0;
        if(i) library_json_literal(out,","); library_json_number(out,meta->reserved_codes[i]);
    }
    library_json_literal(out,"],\"supportedFeatures\":"); library_json_number(out,meta->supported_features);
    library_json_literal(out,",\"monochromeOnly\":"); library_json_bool(out,meta->monochrome_only);
    library_json_literal(out,",\"officialHashes\":{");
    for(u32 i=0;i<meta->official_count;++i) {
        const struct library_snapshot_official *official=meta->official+i;
        if(!official->id || !official->hash || !library_manifest_text(official->id,32,31,1)) return 0;
        for(u32 j=0;j<i;++j) if(library_store_equal(official->id,meta->official[j].id,32)) return 0;
        if(i) library_json_literal(out,","); library_json_text(out,official->id,31);
        library_json_literal(out,":"); library_json_hash(out,official->hash);
    }
    library_json_literal(out,"}}}");
    return !out->failed;
}
#endif
