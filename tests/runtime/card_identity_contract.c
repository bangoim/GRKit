#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
#include "library_card_identity.h"
struct fixture { u8 cid[16],mbr[512],boot[512]; u32 calls; int change; };
static int read_cid(void *p,u8 out[16]) {
    struct fixture *f=p; memcpy(out,f->cid,16);
    if(f->change && f->calls) out[0]^=1;
    return 1;
}
static int read_sector(void *p,u32 lba,u8 out[512]) {
    struct fixture *f=p; ++f->calls;
    assert(lba==0 || lba==2048); memcpy(out,lba?f->boot:f->mbr,512); return 1;
}
#ifdef TEST_GR4
#include "fastboot/targets/gr4-1.21.h"
#else
#include "fastboot/targets/gr3-2.21.h"
#endif
static struct fixture *target_fixture;
static int wrong_driver,wrong_method,bad_cid,read_failure;
static u32 entry_count=2;
static int wrong_filesystem,wrong_fat;
static u32 filesystem_reads,detach_after_admit;
static u32 target_word(u32 address) {
    switch(address) {
#ifdef TEST_GR4
        case 0x5537fc08U: return detach_after_admit && ++filesystem_reads>1U?0:0x59000300U;
        case 0x59000300U: return wrong_filesystem?0:0x53fd39f0U;
        case 0x59000500U: return wrong_fat?0x53fd06a4U:0x53fd3654U;
#else
        case 0x55349e1cU: return 0x59000300U;
        case 0x59000300U: return wrong_filesystem?0:0x53d2d200U;
        case 0x59000500U: return wrong_fat?0:0x53d2ce64U;
#endif
        case 0x590004fcU: return 0; /* Unverified exFAT primary interface. */
        case 0x59000318U: return 0x59000400U;
        case 0x59000400U: return 0x59000500U;
        case RT_CARD_MANAGER: return 0x59000000U;
        case 0x59000014U: return 0x59000100U;
        case 0x59000020U: return entry_count;
        case 0x59000104U: return 'A';
        case 0x59000104U+RT_CARD_ENTRY_SIZE: return 'C';
        case 0x59000100U+RT_CARD_ENTRY_SIZE: return 0x59000200U;
        case 0x59000200U: return wrong_driver?0:RT_CARD_VTABLE;
        case RT_CARD_VTABLE+RT_CARD_READ_SLOT: return wrong_method?0:RT_CARD_READ;
#ifdef TEST_GR4
        /* The same slot used by GR III is a WRITE method on GR IV. */
        case RT_CARD_VTABLE+0x2cU: return 0x53a753b0U;
#endif
        case 0x59000218U: return 8192;
        default: assert(0); return 0;
    }
}
static u8 target_byte(u32 address) {
    assert(address>=RT_CARD_CID && address<=(RT_CARD_CID+15U));
    return address==(RT_CARD_CID+15U)?(bad_cid?0:1):target_fixture->cid[address-RT_CARD_CID];
}
static int target_read(u32 driver,u32 lba,u32 count,u8 *bytes) {
    assert(driver==0x59000200U && count==1);
    assert(!((uintptr_t)bytes&63));
    return !read_failure && read_sector(target_fixture,lba,bytes);
}
#define LIBRARY_CARD_TARGET_WORD(address) target_word(address)
#define LIBRARY_CARD_TARGET_BYTE(address) target_byte(address)
#define LIBRARY_CARD_TARGET_READ(driver,lba,count,bytes) target_read(driver,lba,count,bytes)
#include "library_card_target.h"
#define LIBRARY_OWNER_TARGET_WORD(address) target_word(address)
#define LIBRARY_EXFAT_WORD(address) target_word(address)
#define LIBRARY_EXFAT_LOCK(mutex) ((void)(mutex),0U)
#define LIBRARY_EXFAT_UNLOCK(mutex) ((void)(mutex),0U)
#define LIBRARY_EXFAT_FLUSH(volume) ((void)(volume),1)
#include "library_owner_target.h"
int main(int argc,char **argv) {
    /* The owner state machine has its own fault-injection contract. */
    (void)library_owner_init; (void)library_owner_submit;
    (void)library_owner_poll; (void)library_owner_result;
    assert(!library_owner_target_resident(0));
    assert(argc==2); FILE *file=fopen(argv[1],"rb"); assert(file);
    struct fixture f={0}; assert(fread(f.cid,1,16,file)==16); assert(fread(f.boot,1,512,file)==512);
    assert(fclose(file)==0); char expected[70],actual[70];
    if(!library_card_identity(f.cid,f.boot,expected)) { puts("unsupported"); return 0; }
    puts(expected);
    f.mbr[510]=0x55; f.mbr[511]=0xaa; f.mbr[450]=0x0c;
    f.mbr[455]=8; /* LBA 2048, little endian. */
    f.mbr[459]=0x10; /* 4096 sectors. */
    struct library_card_scratch scratch; u8 evidence[532];
    struct library_card_reader reader={&f,8192,read_cid,read_sector};
    assert(library_card_read_identity(&reader,actual,evidence,&scratch)); assert(!strcmp(expected,actual));
    target_fixture=&f;
    assert(!library_owner_target_admit(0));
    assert(!library_owner_target_completed(0));
    wrong_filesystem=1; assert(library_owner_target_admit(0)); wrong_filesystem=0;
    wrong_fat=1; assert(library_owner_target_admit(0)); wrong_fat=0;
#ifdef TEST_GR4
    detach_after_admit=1; filesystem_reads=0;
    assert(library_owner_target_completed(0));
    detach_after_admit=0;
#else
    (void)filesystem_reads; (void)detach_after_admit;
#endif
    wrong_driver=1; assert(library_owner_target_admit(0)); wrong_driver=0;
    wrong_method=1; assert(library_owner_target_admit(0)); wrong_method=0;
    entry_count=1; assert(library_owner_target_admit(0)); entry_count=2;
    struct library_owner_fs owner_fs=library_owner_target_fs();
    assert(owner_fs.admit_write && owner_fs.durable);
    assert(!owner_fs.admit_write(0) && !owner_fs.durable(0));
    assert(library_card_target_read_evidence(evidence,&scratch));
    assert(library_card_identity(evidence+4,evidence+20,actual)); assert(!strcmp(expected,actual));
    assert(evidence[19]==0);
    wrong_driver=1; assert(!library_card_target_read_evidence(evidence,&scratch)); wrong_driver=0;

    wrong_method=1; assert(!library_card_target_read_evidence(evidence,&scratch)); wrong_method=0;
    bad_cid=1; assert(!library_card_target_read_evidence(evidence,&scratch)); bad_cid=0;
    read_failure=1; assert(!library_card_target_read_evidence(evidence,&scratch)); read_failure=0;
    entry_count=0; assert(!library_card_target_read_evidence(evidence,&scratch));
    entry_count=27; assert(!library_card_target_read_evidence(evidence,&scratch)); entry_count=2;
    f.calls=0; f.change=1; assert(!library_card_read_identity(&reader,actual,evidence,&scratch)); f.change=0;
    u32 lba; f.mbr[466]=0x0c; f.mbr[471]=0x18; f.mbr[475]=1;
    assert(!library_card_volume_lba(f.mbr,8192,&lba));
    f.mbr[466]=0; f.mbr[471]=0; f.mbr[475]=0;
    assert(!library_card_volume_lba(f.mbr,4096,&lba));
    f.mbr[450]=0xee; assert(!library_card_volume_lba(f.mbr,8192,&lba));
    memcpy(f.mbr,f.boot,512); assert(library_card_volume_lba(f.mbr,8192,&lba) && !lba);
    return 0;
}
