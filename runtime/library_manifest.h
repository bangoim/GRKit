#ifndef GRKIT_LIBRARY_MANIFEST_H
#define GRKIT_LIBRARY_MANIFEST_H
#include "library_store.h"

/* Views borrow an immutable owner snapshot. A caller must retain that snapshot
   until all views are released; these pointers never name mutable mailbox data. */
struct library_manifest { const u8 *bytes; u32 entries,listed,loaded; };
struct library_manifest_entry {
    const u8 *id,*name,*hash;
    u32 flags,features;
};
static int library_manifest_text(const u8 *bytes,u32 capacity,u32 maximum,int id)
{
    u32 length=0;
    while(length<capacity && bytes[length]) {
        u8 c=bytes[length];
        if(id ? !((c>='a' && c<='z') || (c>='0' && c<='9') || (length && c=='-'))
              : (c<32 || c>126)) return 0;
        ++length;
    }
    if(!length || length>maximum || length==capacity) return 0;
    for(u32 i=length;i<capacity;++i) if(bytes[i]) return 0;
    return 1;
}
static const u8 *library_manifest_record(const struct library_manifest *view,u32 index)
{ return index<view->entries ? view->bytes+32+96*index : (const u8 *)0; }
static u32 library_manifest_order(const struct library_manifest *view,u32 position)
{
    if(position>=view->listed) return 256;
    const u8 *p=view->bytes+32+96*view->entries+2*position;
    return (u32)p[0]|((u32)p[1]<<8);
}
static int library_manifest_get(const struct library_manifest *view,u32 index,struct library_manifest_entry *entry)
{
    if(!view || !entry) return 0;
    const u8 *p=library_manifest_record(view,index);
    if(!p) return 0;
    entry->id=p; entry->name=p+32; entry->hash=p+52;
    entry->flags=library_store_word(p+84); entry->features=library_store_word(p+88);
    return 1;
}
/* Match the Rust format-1 codec, including tombstones and exact order coverage.
   Hash/content agreement, feature support, official provenance and dependencies
   are checked against content/runtime state separately, never inferred here. */
static int library_manifest_open(const u8 *bytes,u32 length,struct library_manifest *out)
{
    if(!out || !library_store_manifest(bytes,length)) return 0;
    struct library_manifest view={bytes,library_store_word(bytes+16),library_store_word(bytes+20),0};
    u8 seen[32]={0};
    u32 active=0;
    for(u32 i=0;i<view.entries;++i) {
        const u8 *p=library_manifest_record(&view,i);
        u32 flags=library_store_word(p+84);
        if(!library_manifest_text(p,32,31,1) || !library_manifest_text(p+32,20,16,0) ||
           (flags&~31U) || ((flags>>3)&3U)==3 || library_store_word(p+92) ||
           ((flags&2U) && (flags&1U))) return 0;
        if(i) {
            const u8 *previous=library_manifest_record(&view,i-1);
            u32 j=0;
            while(j<32 && previous[j]==p[j]) ++j;
            if(j==32 || previous[j]>=p[j]) return 0;
        }
        if(!(flags&2U)) { ++active; if(flags&1U) ++view.loaded; }
    }
    if(active!=view.listed || view.loaded>40) return 0;
    for(u32 i=0;i<view.listed;++i) {
        u32 index=library_manifest_order(&view,i);
        if(index>=view.entries || (seen[index>>3]&(1U<<(index&7U))) || (library_store_word(library_manifest_record(&view,index)+84)&2U)) return 0;
        seen[index>>3]|=(u8)(1U<<(index&7U));
    }
    *out=view;
    return 1;
}
#endif
