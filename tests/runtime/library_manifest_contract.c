#include <assert.h>
#include <stdint.h>
#include <string.h>
typedef uint32_t u32;
typedef uint8_t u8;
#include "library_manifest.h"
static u8 bytes[LIBRARY_STORE_MANIFEST_MAX],saved[LIBRARY_STORE_MANIFEST_MAX];
static u32 size;
static struct library_manifest view;
static void checksum(void) { library_store_put(bytes+24,library_store_crc(bytes,size,24,4)); }
static void make(u32 entries,u32 active,u32 enabled)
{
    memset(bytes,0,sizeof(bytes));
    memcpy(bytes,"GRKLIB01",8); size=32+entries*96+active*2;
    library_store_put(bytes+8,1); library_store_put(bytes+12,size);
    library_store_put(bytes+16,entries); library_store_put(bytes+20,active);
    for(u32 i=0;i<entries;++i) {
        u8 *p=bytes+32+i*96;
        p[0]='a'+i/100; p[1]='0'+i/10%10; p[2]='0'+i%10;
        memcpy(p+32,"Look",4); memset(p+52,(int)i,32);
        library_store_put(p+84,8U|(i>=active?2U:i<enabled?1U:0U));
    }
    for(u32 i=0;i<active;++i) bytes[32+entries*96+2*i]=(u8)(active-i-1);
    checksum();
}
static void reject(u32 offset,u8 value)
{
    memcpy(saved,bytes,size); bytes[offset]=value; checksum();
    assert(!library_manifest_open(bytes,size,&view)); memcpy(bytes,saved,size);
}
int main(void)
{
    make(0,0,0); assert(library_manifest_open(bytes,size,&view));
    make(256,64,40); assert(library_manifest_open(bytes,size,&view));
    assert(view.entries==256 && view.listed==64 && view.loaded==40);
    assert(library_manifest_order(&view,0)==63 && library_manifest_order(&view,64)==256);
    struct library_manifest_entry entry;
    assert(library_manifest_get(&view,63,&entry) && !memcmp(entry.id,"a63",4));
    assert(!library_manifest_get(&view,256,&entry));
    reject(32,'-'); reject(32,'A'); reject(32+4,'x');
    reject(32+32,31); reject(32+32,127); reject(32+32+17,'x');
    reject(32+84,32); reject(32+84,24); reject(32+92,1);
    reject(32+64*96+84,11); /* removed and enabled */
    reject(32+256*96,64); /* tombstone in order */
    reject(32+256*96+2,63); /* duplicate order */
    reject(32+96+2,'0'); /* duplicate ID */
    make(64,64,41); assert(!library_manifest_open(bytes,size,&view));
    make(65,65,40); assert(!library_manifest_open(bytes,size,&view));
    make(2,2,1); library_store_put(bytes+20,1); size-=2;
    library_store_put(bytes+12,size); checksum(); assert(!library_manifest_open(bytes,size,&view));
    make(2,2,1); bytes[24]^=1; assert(!library_manifest_open(bytes,size,&view));
    assert(!library_manifest_open(bytes,31,&view));
    return 0;
}
