#include <assert.h>
#include <stdint.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
#define LOOK_TELEMETRY_BARRIER() __atomic_thread_fence(__ATOMIC_SEQ_CST)
#include "library_pending.h"
#include "library_seed.h"
#include "grf.h"
static u8 old_bytes[LIBRARY_STORE_MANIFEST_MAX],new_bytes[LIBRARY_STORE_MANIFEST_MAX],grf[GRKIT_GRF_SIZE];
static struct library_manifest old,next;
static u32 action,target_index;
static int validate(const u8 *p,u32 n) { return grkit_grf_validate(p,n); }
static void profile(const char *id,const char *name)
{
    memset(grf,0,sizeof(grf)); memcpy(grf,"GRFX0001",8);
    library_store_put(grf+GRKIT_GRF_BYTES,sizeof(grf));
    strcpy((char *)grf+GRKIT_GRF_ID,id); strcpy((char *)grf+GRKIT_GRF_NAME,name);
    library_store_put(grf+GRKIT_GRF_CRC,library_store_crc(grf,sizeof(grf),GRKIT_GRF_CRC,4));
    assert(validate(grf,sizeof(grf)));
}
static void open_manifest(u8 *bytes,struct library_manifest *out)
{
    u32 length=library_store_word(bytes+12);
    library_store_put(bytes+24,library_store_crc(bytes,length,24,4));
    assert(library_manifest_open(bytes,length,out));
}
static void reset(u32 origin)
{
    struct library_seed seed; u32 length;
    profile("sample","Sample");
    assert(library_seed_begin(&seed,old_bytes,sizeof(old_bytes)));
    assert(library_seed_add(&seed,grf+GRKIT_GRF_ID,grf,sizeof(grf),1,0,validate));
    assert(library_seed_finish(&seed,&length));
    library_store_put(old_bytes+32+84,1|(origin<<3)); open_manifest(old_bytes,&old);
    memcpy(new_bytes,old_bytes,length); open_manifest(new_bytes,&next);
}
static int classify(u32 content) { return library_candidate_classify(&old,&next,content?grf:0,content?sizeof(grf):0,validate,&action,&target_index); }
static void replace_content(void)
{
    profile("sample","Renamed"); memcpy(new_bytes+32+32,grf+GRKIT_GRF_NAME,20);
    assert(library_sha256(grf,sizeof(grf),new_bytes+32+52)); open_manifest(new_bytes,&next);
}
static void remove_entry(u8 *bytes,struct library_manifest *manifest)
{
    library_store_put(bytes+32+84,(library_store_word(bytes+32+84)&~1U)|2U);
    library_store_put(bytes+12,128); library_store_put(bytes+20,0); open_manifest(bytes,manifest);
}
int main(void)
{
    reset(1); assert(classify(0) && action==LIBRARY_CANDIDATE_METADATA);
    library_store_put(new_bytes+32+84,8); open_manifest(new_bytes,&next);
    assert(classify(0) && action==LIBRARY_CANDIDATE_METADATA);
    reset(1); replace_content(); assert(classify(1) && action==LIBRARY_CANDIDATE_REPLACE && target_index==0);
    assert(!classify(0)); grf[GRKIT_GRF_ICON]^=1; assert(!classify(1));
    reset(2); replace_content(); assert(!classify(1));
    reset(0); replace_content(); assert(!classify(1));
    reset(0); library_store_put(new_bytes+32+84,17); open_manifest(new_bytes,&next);
    assert(classify(0) && action==LIBRARY_CANDIDATE_METADATA);
    reset(2); library_store_put(new_bytes+32+84,9); open_manifest(new_bytes,&next); assert(!classify(0));
    reset(1); remove_entry(new_bytes,&next); assert(classify(0));
    reset(1); remove_entry(old_bytes,&old); assert(classify(1) && action==LIBRARY_CANDIDATE_ADD);
    reset(2); remove_entry(old_bytes,&old); assert(classify(1) && action==LIBRARY_CANDIDATE_ADD);
    replace_content(); assert(!classify(1));
    reset(0); remove_entry(old_bytes,&old); assert(!classify(1));
    reset(1); library_store_put(new_bytes+12,32); library_store_put(new_bytes+16,0); library_store_put(new_bytes+20,0);
    open_manifest(new_bytes,&next); assert(!classify(0)); /* Identity cannot vanish. */
    reset(1); library_store_put(old_bytes+12,32); library_store_put(old_bytes+16,0); library_store_put(old_bytes+20,0);
    open_manifest(old_bytes,&old); assert(classify(1) && action==LIBRARY_CANDIDATE_ADD);
    reset(1);
    library_store_put(old_bytes+32+84,8); open_manifest(old_bytes,&old);
    struct look_telemetry telemetry={0}; telemetry.count=1;
    memcpy(telemetry.entries[0].id,"sample",6);
    assert(library_pending_index(&old,&next,&telemetry,&target_index) && target_index==0);
    telemetry.entries[0].loaded=1;
    assert(library_pending_index(&old,&next,&telemetry,&target_index) && target_index==256);
    telemetry.count=0; assert(!library_pending_index(&old,&next,&telemetry,&target_index));
    telemetry.count=2; telemetry.entries[1]=telemetry.entries[0];
    assert(!library_pending_index(&old,&next,&telemetry,&target_index));
    /* Loading one pending row cannot silently enable another pending row or
       reorder the whole catalog through the one-record publication action. */
    reset(1);
    library_store_put(old_bytes+32+84,8);
    memcpy(old_bytes+128,old_bytes+32,96);
    memset(old_bytes+128,0,32); memcpy(old_bytes+128,"second",6); old_bytes[128+52]^=1;
    library_store_put(old_bytes+12,228); library_store_put(old_bytes+16,2); library_store_put(old_bytes+20,2);
    old_bytes[224]=old_bytes[225]=old_bytes[227]=0; old_bytes[226]=1; open_manifest(old_bytes,&old);
    memcpy(new_bytes,old_bytes,228); library_store_put(new_bytes+32+84,9); library_store_put(new_bytes+128+84,9);
    open_manifest(new_bytes,&next);
    memset(&telemetry,0,sizeof(telemetry)); telemetry.count=2;
    memcpy(telemetry.entries[0].id,"sample",6); memcpy(telemetry.entries[1].id,"second",6);
    assert(!library_pending_index(&old,&next,&telemetry,&target_index));
    library_store_put(new_bytes+128+84,8); open_manifest(new_bytes,&next);
    assert(library_pending_index(&old,&next,&telemetry,&target_index) && target_index==0);
    new_bytes[224]=1; new_bytes[226]=0; open_manifest(new_bytes,&next);
    assert(!library_pending_index(&old,&next,&telemetry,&target_index));
    return 0;
}
