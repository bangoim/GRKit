#include <assert.h>
#include <stdint.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
#define LOOK_TELEMETRY_BARRIER() ((void)0)
#include "library_telemetry_metadata.h"
static u8 bytes[324],candidate[324];
static struct library_manifest manifest;
static struct look_telemetry telemetry;
static struct library_telemetry_metadata scratch;
static struct library_snapshot_metadata meta;
static void setup(void)
{
    memset(bytes,0,sizeof(bytes)); memcpy(bytes,"GRKLIB01",8);
    library_store_put(bytes+8,1); library_store_put(bytes+12,sizeof(bytes));
    library_store_put(bytes+16,3); library_store_put(bytes+20,2);
    const char *ids[]={"alpha","beta","retired"};
    for(u32 i=0;i<3;++i) {
        u8 *p=bytes+32+96*i; memcpy(p,ids[i],strlen(ids[i])); memcpy(p+32,"Name",4);
        memset(p+52,(int)i+1,32); library_store_put(p+84,i==2?10:9);
    }
    bytes[322]=1;
    library_store_put(bytes+24,library_store_crc(bytes,sizeof(bytes),24,4));
    assert(library_manifest_open(bytes,sizeof(bytes),&manifest));
    memset(&telemetry,0,sizeof(telemetry)); telemetry.revision=1; telemetry.count=2; telemetry.record_count=3;
    for(u32 i=0;i<3;++i) {
        memcpy(telemetry.records[i].id,ids[i],strlen(ids[i]));
        telemetry.records[i].code=0x8000U|library_dependency_code(telemetry.records[i].id);
        memset(telemetry.records[i].hash,(int)i+1,32);
        if(i<2) { memcpy(telemetry.entries[i].id,telemetry.records[i].id,32);
            telemetry.entries[i].loaded=telemetry.entries[i].enabled=1; telemetry.entries[i].position=i; }
    }
    telemetry.recipe_ids[63]=42; telemetry.entries[1].recipes[1]=0x80000000U;
    telemetry.entries[0].dial=1;
    telemetry.selected=1234; memcpy(telemetry.selected_id,"beta",4);
    memset(&meta,0,sizeof(meta)); meta.generation=7;
}
static u32 validate(void) { return library_snapshot_from_telemetry(&manifest,&telemetry,&scratch,&meta); }
int main(void)
{
    setup(); assert(validate()==LIBRARY_TELEMETRY_OK);
    assert(meta.generation==7 && meta.selected==1 && meta.loaded_count==2 && meta.reserved_count==0);
    assert(meta.dependencies->count==2 && meta.dependencies->items[1].id==42);
    setup(); memset(telemetry.records[2].id,0,32); memcpy(telemetry.records[2].id,"unknown",7);
    telemetry.records[2].code=0x8000U|library_dependency_code(telemetry.records[2].id);
    assert(validate()==LIBRARY_TELEMETRY_OK && meta.reserved_count==1);
    assert(meta.reserved_codes[0]==library_dependency_code((const u8 *)"unknown"));
    setup(); telemetry.entries[0].enabled=0;
    assert(validate()==LIBRARY_TELEMETRY_NEEDS_RECONCILE && !meta.manifest);
    assert(library_telemetry_reconcile(&manifest,&telemetry,candidate,sizeof(candidate)));
    assert(!memcmp(candidate+32+52,bytes+32+52,32));
    assert(!memcmp(candidate+32+192,bytes+32+192,96));
    assert(library_store_word(candidate+32+84)==8 && library_store_word(bytes+32+84)==9);
    setup(); telemetry.entries[0].position=1; telemetry.entries[1].position=0;
    assert(validate()==LIBRARY_TELEMETRY_NEEDS_RECONCILE);
    assert(library_telemetry_reconcile(&manifest,&telemetry,candidate,sizeof(candidate)));
    assert(candidate[320]==1 && candidate[322]==0);
    setup(); telemetry.recipe_ids[63]=0; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); telemetry.recipe_ids[0]=42; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); telemetry.entries[0].recipes[1]=0x80000000U; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); telemetry.entries[1].dial=1; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); telemetry.entries[1].position=0; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); memcpy(telemetry.entries[1].id,telemetry.entries[0].id,32); assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); telemetry.records[1].code=telemetry.records[0].code; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); telemetry.records[0].code=0x8001; assert(validate()==LIBRARY_TELEMETRY_OK);
    setup(); telemetry.entries[0].loaded=0; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); telemetry.seq=1; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); telemetry.flags=1; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); telemetry.count=1; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); memcpy(telemetry.selected_id,telemetry.records[2].id,32); assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); telemetry.selected=0; memset(telemetry.selected_id,0,32); assert(validate()==LIBRARY_TELEMETRY_OK && meta.selected==256);
    setup(); assert(library_snapshot_from_telemetry(&manifest,&telemetry,(struct library_telemetry_metadata *)&telemetry,&meta)==LIBRARY_TELEMETRY_INVALID);
    setup(); assert(!library_telemetry_reconcile(&manifest,&telemetry,bytes+1,sizeof(bytes)));
    setup(); assert(!library_telemetry_reconcile(&manifest,&telemetry,candidate,sizeof(candidate)-1));
    setup(); telemetry.entries[0].dial=1U<<GRKIT_DIAL_USER_MODES; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); telemetry.record_count=41; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); telemetry.records[0].hash[0]^=1; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); telemetry.records[0].code=1; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); telemetry.entries[0].loaded=2; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    setup(); telemetry.entries[0].enabled=2; assert(validate()==LIBRARY_TELEMETRY_INVALID);
    return 0;
}
