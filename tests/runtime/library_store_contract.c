#include <assert.h>
#include <stdint.h>
#include <string.h>
typedef uint32_t u32;
typedef uint8_t u8;
#include "library_store.h"

struct card {
    u8 disk[2][LIBRARY_STORE_SLOT_BYTES];
    u8 cache[2][LIBRARY_STORE_SLOT_BYTES];
    long fail_after;
    u32 written,syncs,fail_sync;
    int immediate,failed_sync_persists,fail_readback,corrupt_readback,allow_new;
};
static struct card card,baseline;
static u8 scratch[LIBRARY_STORE_MANIFEST_MAX],old_manifest[130],new_manifest[130];
static struct library_store_view view;

static int read_at(void *owner,u32 slot,u32 offset,u8 *bytes,u32 count)
{
    struct card *c=owner;
    assert(slot<2 && offset<=LIBRARY_STORE_SLOT_BYTES && count<=LIBRARY_STORE_SLOT_BYTES-offset);
    if(c->fail_readback && c->syncs>=3) return 1;
    memcpy(bytes,c->cache[slot]+offset,count);
    if(c->corrupt_readback && c->syncs>=3 && slot==1 && offset==32 && count) bytes[count-1]^=1;
    return 0;
}
static int write_at(void *owner,u32 slot,u32 offset,const u8 *bytes,u32 count)
{
    struct card *c=owner;
    assert(slot<2 && offset<=LIBRARY_STORE_SLOT_BYTES && count<=LIBRARY_STORE_SLOT_BYTES-offset);
    for(u32 i=0;i<count;++i) {
        if(c->fail_after>=0 && c->written==(u32)c->fail_after) return 1;
        c->cache[slot][offset+i]=bytes[i];
        if(c->immediate) c->disk[slot][offset+i]=bytes[i];
        ++c->written;
    }
    return 0;
}
static int sync_slot(void *owner,u32 slot)
{
    struct card *c=owner;
    ++c->syncs;
    if(c->syncs==c->fail_sync && !c->failed_sync_persists) return 1;
    memcpy(c->disk[slot],c->cache[slot],LIBRARY_STORE_SLOT_BYTES);
    return c->syncs==c->fail_sync;
}
/* The fixture models immutable content already persisted by the RTOS owner.
   Production must verify SHA-256 and full entry semantics, not just this tag. */
static int contents_ready(void *owner,const u8 *bytes,u32 count)
{
    struct card *c=owner;
    assert(library_store_manifest(bytes,count));
    assert(count==sizeof(old_manifest));
    return bytes[64]=='N' && !c->allow_new;
}
static struct library_store_io io={&card,read_at,write_at,sync_slot,contents_ready};

static void manifest(u8 *bytes,char name)
{
    memset(bytes,0,130);
    memcpy(bytes,"GRKLIB01",8);
    library_store_put(bytes+8,1);
    library_store_put(bytes+12,130);
    library_store_put(bytes+16,1);
    library_store_put(bytes+20,1);
    memcpy(bytes+32,"look",4);
    bytes[64]=(u8)name;
    memset(bytes+84,name,32);
    library_store_put(bytes+116,1U|(1U<<3)); /* enabled custom */
    library_store_put(bytes+24,library_store_crc(bytes,130,24,4));
}
static void reset_faults(void)
{
    card.fail_after=-1; card.written=0; card.syncs=0; card.fail_sync=0;
    card.immediate=0; card.failed_sync_persists=0; card.fail_readback=0; card.corrupt_readback=0; card.allow_new=1;
}
static void power_loss(void)
{
    memcpy(card.cache,card.disk,sizeof(card.cache));
    reset_faults();
}
static void assert_generation_pair(u32 generation,const u8 *old,const u8 *next)
{
    assert(library_store_recover(&io,scratch,sizeof(scratch),&view)==LIBRARY_STORE_OK);
    assert(view.generation==generation || view.generation==generation+1);
    assert(!memcmp(scratch,view.generation==generation?old:next,130));
}
static void assert_old_or_new(void) { assert_generation_pair(1,old_manifest,new_manifest); }
static void power_cut_at_each_written_byte(u32 generation,const u8 *old,const u8 *next)
{
    const u32 total=16+32+130+16;
    for(int immediate=0;immediate<2;++immediate) for(u32 cut=0;cut<=total;++cut) {
        card=baseline;
        reset_faults();
        card.immediate=immediate;
        card.fail_after=(long)cut;
        struct library_store_view result={99,99,99,99};
        int status=library_store_commit(&io,generation,next,130,scratch,sizeof(scratch),&result);
        if(cut<total) { assert(status==LIBRARY_STORE_IO); assert(result.generation==99); }
        else { assert(status==LIBRARY_STORE_OK); assert(result.generation==generation+1); }
        power_loss();
        assert_generation_pair(generation,old,next);
    }
}
static void failed_sync_never_reports_success(void)
{
    for(u32 fail=1;fail<=3;++fail) for(int persisted=0;persisted<2;++persisted) {
        card=baseline; reset_faults();
        card.fail_sync=fail; card.failed_sync_persists=persisted;
        struct library_store_view result={99,99,99,99};
        assert(library_store_commit(&io,1,new_manifest,130,scratch,sizeof(scratch),&result)==LIBRARY_STORE_IO);
        assert(result.generation==99);
        power_loss();
        assert_old_or_new();
        if(fail==3 && persisted) assert(view.generation==2);
    }
}
static void failures_and_admission(void)
{
    card=baseline; reset_faults();
    card.allow_new=0;
    assert(library_store_commit(&io,1,new_manifest,130,scratch,sizeof(scratch),&view)==LIBRARY_STORE_CONTENT_NOT_READY);
    assert(!card.written);
    card.allow_new=1;
    assert(library_store_commit(&io,2,new_manifest,130,scratch,sizeof(scratch),&view)==LIBRARY_STORE_STALE);
    assert(!card.written);
    assert(library_store_seed(&io,new_manifest,130,scratch,sizeof(scratch),&view)==LIBRARY_STORE_NOT_EMPTY);
    assert(!card.written);
    card.fail_readback=1;
    struct library_store_view result={99,99,99,99};
    assert(library_store_commit(&io,1,new_manifest,130,scratch,sizeof(scratch),&result)==LIBRARY_STORE_IO);
    assert(result.generation==99);
    power_loss();
    assert(library_store_recover(&io,scratch,sizeof(scratch),&view)==LIBRARY_STORE_OK);
    assert(view.generation==2);
    card=baseline; reset_faults();
    card.corrupt_readback=1;
    result.generation=99;
    assert(library_store_commit(&io,1,new_manifest,130,scratch,sizeof(scratch),&result)==LIBRARY_STORE_RECOVERY_REQUIRED);
    assert(result.generation==99);
    power_loss();
    assert(library_store_recover(&io,scratch,sizeof(scratch),&view)==LIBRARY_STORE_OK);
    assert(view.generation==2);
    card.allow_new=0;
    /* A valid newer manifest with missing immutable contents needs recovery.
       It must not quietly select the older library. */
    assert(library_store_recover(&io,scratch,sizeof(scratch),&view)==LIBRARY_STORE_RECOVERY_REQUIRED);
    power_loss();
    card.cache[0][0]^=1; card.cache[1][0]^=1;
    assert(library_store_recover(&io,scratch,sizeof(scratch),&view)==LIBRARY_STORE_RECOVERY_REQUIRED);
}
static void generation_rules(void)
{
    card=baseline; reset_faults();
    /* A duplicate generation in both slots is not a valid transaction state. */
    memcpy(card.cache[1],card.cache[0],LIBRARY_STORE_SLOT_BYTES);
    assert(library_store_recover(&io,scratch,sizeof(scratch),&view)==LIBRARY_STORE_RECOVERY_REQUIRED);
    card=baseline; reset_faults();
    u8 *header=card.cache[0],*commit=header+LIBRARY_STORE_COMMIT_OFFSET;
    library_store_put(header+8,0xffffffffU);
    library_store_put(header+20,library_store_crc(header,32,20,4));
    library_store_put(commit+4,0xffffffffU);
    library_store_put(commit+12,library_store_crc(commit,16,12,4));
    assert(library_store_recover(&io,scratch,sizeof(scratch),&view)==LIBRARY_STORE_OK);
    assert(view.generation==0xffffffffU);
    assert(library_store_commit(&io,0xffffffffU,new_manifest,130,scratch,sizeof(scratch),&view)==LIBRARY_STORE_GENERATION_EXHAUSTED);
    assert(!card.written);
}
int main(void)
{
    manifest(old_manifest,'O'); manifest(new_manifest,'N');
    memset(&card,0,sizeof(card)); reset_faults();
    assert(library_store_recover(&io,scratch,sizeof(scratch),&view)==LIBRARY_STORE_RECOVERY_REQUIRED);
    assert(library_store_seed(&io,old_manifest,130,scratch,sizeof(scratch),&view)==LIBRARY_STORE_OK);
    assert(view.generation==1 && view.slot==0 && !memcmp(scratch,old_manifest,130));
    baseline=card;
    power_cut_at_each_written_byte(1,old_manifest,new_manifest);
    failed_sync_never_reports_success();
    failures_and_admission();
    generation_rules();
    /* Reuse a previously committed inactive slot, including its old marker.
       This exercises invalidation rather than only writing a blank slot. */
    card=baseline; reset_faults();
    assert(library_store_commit(&io,1,new_manifest,130,scratch,sizeof(scratch),&view)==LIBRARY_STORE_OK);
    baseline=card;
    power_cut_at_each_written_byte(2,new_manifest,old_manifest);
    return 0;
}
