#include <assert.h>
#include <stdint.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
#include "library_content.h"
#include "grf.h"
static int validate(const u8 *bytes,u32 length) { return grkit_grf_validate(bytes,length); }
static u8 grf[GRKIT_GRF_SIZE],manifest[130],hash[32];
static void crc(void) { library_store_put(grf+GRKIT_GRF_CRC,library_store_crc(grf,sizeof(grf),GRKIT_GRF_CRC,4)); }
int main(void)
{
    memcpy(grf,"GRFX0001",8); library_store_put(grf+GRKIT_GRF_BYTES,sizeof(grf));
    memcpy(grf+GRKIT_GRF_ID,"sample",6); memcpy(grf+GRKIT_GRF_NAME,"Sample",6); crc();
    assert(validate(grf,sizeof(grf))); assert(library_sha256(grf,sizeof(grf),hash));
    memcpy(manifest,"GRKLIB01",8); library_store_put(manifest+8,1); library_store_put(manifest+12,130);
    library_store_put(manifest+16,1); library_store_put(manifest+20,1);
    memcpy(manifest+32,"sample",6); memcpy(manifest+64,"Sample",6); memcpy(manifest+84,hash,32);
    library_store_put(manifest+116,9); library_store_put(manifest+24,library_store_crc(manifest,130,24,4));
    struct library_manifest view; struct library_manifest_entry entry;
    assert(library_manifest_open(manifest,130,&view)); assert(library_manifest_get(&view,0,&entry));
    assert(library_content_matches(&entry,grf,sizeof(grf),validate));
    assert(!library_content_matches(&entry,grf,sizeof(grf)-1,validate));
    entry.flags|=2; assert(!library_content_matches(&entry,grf,sizeof(grf),validate)); entry.flags&=~2U;
    entry.flags^=4; assert(!library_content_matches(&entry,grf,sizeof(grf),validate)); entry.flags^=4;
    entry.features=1; assert(!library_content_matches(&entry,grf,sizeof(grf),validate)); entry.features=0;
    grf[GRKIT_GRF_NAME]='s'; crc(); assert(!library_content_matches(&entry,grf,sizeof(grf),validate));
    grf[GRKIT_GRF_NAME]='S'; grf[GRKIT_GRF_ICON]=1; crc();
    assert(validate(grf,sizeof(grf))); assert(!library_content_matches(&entry,grf,sizeof(grf),validate));
    return 0;
}
