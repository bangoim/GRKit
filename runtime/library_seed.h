#ifndef GRKIT_LIBRARY_SEED_H
#define GRKIT_LIBRARY_SEED_H
#include "library_content.h"
#include "library_dependencies.h"
#define LIBRARY_SEED_INVALID 1U
#define LIBRARY_SEED_CAPACITY 2U
#define LIBRARY_SEED_CONFLICT 3U
struct library_seed {
    u8 *bytes;
    u32 capacity,count,enabled,error,finished,length;
};
static int library_seed_overlap(const void *a,u32 an,const void *b,u32 bn)
{
    __UINTPTR_TYPE__ x=(__UINTPTR_TYPE__)a,y=(__UINTPTR_TYPE__)b,maximum=~(__UINTPTR_TYPE__)0;
    return an>maximum-x || bn>maximum-y || (x<y+bn && y<x+an);
}
/* The owner supplies only the actually listed legacy Looks, in their current
   menu order. Files hidden by package selections do not become new entries.
   Origin starts Unknown: a filename or installed record is not signed proof. */
static int library_seed_begin(struct library_seed *seed,u8 *bytes,u32 capacity)
{
    if(!seed || !bytes || capacity<32 || capacity>LIBRARY_STORE_MANIFEST_MAX || library_seed_overlap(seed,sizeof(*seed),bytes,capacity)) return 0;
    seed->bytes=bytes; seed->capacity=capacity; seed->count=seed->enabled=seed->error=seed->finished=seed->length=0;
    for(u32 i=0;i<32;++i) bytes[i]=0;
    return 1;
}
static int library_seed_fail(struct library_seed *seed,u32 reason)
{ seed->error=reason; return 0; }
static int library_seed_add(struct library_seed *seed,const u8 expected_id[32],const u8 *grf,u32 length,
                            u32 enabled,u32 runtime_code,int (*validate_grf)(const u8 *,u32))
{
    if(!seed || !seed->bytes || seed->error || seed->finished) return 0;
    if(!expected_id || !grf || !validate_grf || enabled>1 ||
       library_seed_overlap(grf,length,seed->bytes,seed->capacity) ||
       library_seed_overlap(expected_id,32,seed->bytes,seed->capacity) ||
       (length!=GRKIT_GRF_SIZE && length!=GRKIT_GRF_MAX_SIZE) || !validate_grf(grf,length) ||
       !library_store_equal(expected_id,grf+GRKIT_GRF_ID,32)) return library_seed_fail(seed,LIBRARY_SEED_INVALID);
    if(seed->count>=64 || seed->enabled+enabled>40 || 32+(seed->count+1)*98>seed->capacity)
        return library_seed_fail(seed,LIBRARY_SEED_CAPACITY);
    u32 code=library_dependency_code(expected_id);
    if(runtime_code && (runtime_code>65535U || !(runtime_code&0x8000U))) return library_seed_fail(seed,LIBRARY_SEED_CONFLICT);
    for(u32 i=0;i<seed->count;++i) {
        const u8 *old=seed->bytes+32+96*i;
        if(library_store_equal(old,expected_id,32) || library_dependency_code(old)==code) return library_seed_fail(seed,LIBRARY_SEED_CONFLICT);
    }
    u8 *record=seed->bytes+32+96*seed->count;
    for(u32 i=0;i<96;++i) record[i]=0;
    for(u32 i=0;i<32;++i) record[i]=expected_id[i];
    for(u32 i=0;i<16;++i) record[32+i]=grf[GRKIT_GRF_NAME+i];
    if(!library_sha256(grf,length,record+52)) return library_seed_fail(seed,LIBRARY_SEED_INVALID);
    library_store_put(record+84,enabled|(library_store_word(grf+GRKIT_GRF_FLAGS)?4U:0U));
    library_store_put(record+88,library_store_word(grf+GRKIT_GRF_FEATURES));
    /* Temporary ordinal, removed before publishing the canonical manifest. */
    library_store_put(record+92,seed->count);
    seed->enabled+=enabled; ++seed->count;
    return 1;
}
static int library_seed_id_before(const u8 *left,const u8 *right)
{
    for(u32 i=0;i<32;++i) if(left[i]!=right[i]) return left[i]<right[i];
    return 0;
}
static int library_seed_finish(struct library_seed *seed,u32 *length)
{
    if(!seed || !length || seed->error || !seed->bytes ||
       library_seed_overlap(length,sizeof(*length),seed->bytes,seed->capacity) ||
       library_seed_overlap(length,sizeof(*length),seed,sizeof(*seed))) return 0;
    if(seed->finished) { *length=seed->length; return 1; }
    for(u32 i=1;i<seed->count;++i) {
        for(u32 j=i;j && library_seed_id_before(seed->bytes+32+96*j,seed->bytes+32+96*(j-1));--j) {
            u8 *left=seed->bytes+32+96*(j-1),*right=left+96;
            for(u32 k=0;k<96;++k) { u8 byte=left[k]; left[k]=right[k]; right[k]=byte; }
        }
    }
    u32 order=32+96*seed->count;
    for(u32 i=0;i<seed->count;++i) {
        u8 *record=seed->bytes+32+96*i;
        u32 ordinal=library_store_word(record+92);
        seed->bytes[order+2*ordinal]=(u8)i; seed->bytes[order+2*ordinal+1]=0;
        library_store_put(record+92,0);
    }
    seed->length=32+98*seed->count;
    for(u32 i=0;i<8;++i) seed->bytes[i]=(u8)"GRKLIB01"[i];
    library_store_put(seed->bytes+8,1); library_store_put(seed->bytes+12,seed->length);
    library_store_put(seed->bytes+16,seed->count); library_store_put(seed->bytes+20,seed->count);
    library_store_put(seed->bytes+24,library_store_crc(seed->bytes,seed->length,24,4));
    struct library_manifest view;
    if(!library_manifest_open(seed->bytes,seed->length,&view)) return library_seed_fail(seed,LIBRARY_SEED_INVALID);
    seed->finished=1; *length=seed->length; return 1;
}
#endif
