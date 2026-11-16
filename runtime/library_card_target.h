#ifndef GRKIT_LIBRARY_CARD_TARGET_H
#define GRKIT_LIBRARY_CARD_TARGET_H
/* GR III 2.21 and GR IV 1.21 adapters. The resident task owns these reads.
   Target addresses come from the decoded RTOS and the checked SdDriver vtable, not Linux or HTTP.
   Other targets remain unavailable until their acquisition paths are verified. */
#include "library_card_identity.h"
#if defined(RT_CARD_MANAGER) && defined(RT_CARD_ENTRY_SIZE) && defined(RT_CARD_VTABLE) && defined(RT_CARD_READ) && defined(RT_CARD_READ_SLOT) && defined(RT_CARD_CID)
#ifndef LIBRARY_CARD_TARGET_WORD
#define LIBRARY_CARD_TARGET_WORD(address) (*(volatile u32 *)(__UINTPTR_TYPE__)(address))
#endif
#ifndef LIBRARY_CARD_TARGET_BYTE
#define LIBRARY_CARD_TARGET_BYTE(address) (*(volatile u8 *)(__UINTPTR_TYPE__)(address))
#endif
#ifndef LIBRARY_CARD_TARGET_READ
#define LIBRARY_CARD_TARGET_READ(driver,lba,count,bytes) \
    (((int (*)(u32,u32,u32,u8 *))(__UINTPTR_TYPE__)RT_CARD_READ)(driver,lba,count,bytes))
#endif
struct library_card_target { u32 driver,capacity; };
static int library_card_target_pointer(u32 address)
{ return address>=0x53000000U && address<0x70000000U && !(address&3U); }
static u32 library_card_target_word(u32 address)
{ return LIBRARY_CARD_TARGET_WORD(address); }
static int library_card_target_current(struct library_card_target *target)
{
    u32 manager=library_card_target_word(RT_CARD_MANAGER);
    if(!library_card_target_pointer(manager)) return 0;
    u32 entries=library_card_target_word(manager+0x14U);
    u32 count=library_card_target_word(manager+0x20U);
    if(!library_card_target_pointer(entries) || !count || count>26U ||
       entries>0x70000000U-count*RT_CARD_ENTRY_SIZE) return 0;
    u32 driver=0;
    for(u32 i=0;i<count;++i) {
        u32 entry=entries+i*RT_CARD_ENTRY_SIZE;
        if((library_card_target_word(entry+4U)&255U)!='C') continue;
        if(driver) return 0;
        driver=library_card_target_word(entry);
        if(!library_card_target_pointer(driver)) return 0;
    }
    if(!driver || library_card_target_word(driver)!=RT_CARD_VTABLE ||
       library_card_target_word(RT_CARD_VTABLE+RT_CARD_READ_SLOT)!=RT_CARD_READ) return 0;
    u32 capacity=library_card_target_word(driver+0x18U);
    if(!capacity) return 0;
    if(target->driver && (driver!=target->driver || capacity!=target->capacity)) return 0;
    target->driver=driver; target->capacity=capacity; return 1;
}
static int library_card_target_cid(void *context,u8 cid[16])
{
    struct library_card_target *target=context;
    if(!library_card_target_current(target)) return 0;
    /* GR III: getter 530ac8c4 / response unpacker 530af764.
       GR IV: getter 530c2ad4 / response unpacker 530c5974.
       Both copy the 136-bit SDHCI response and force its last byte to 01.
       SDHCI omits CRC7/end bits: normalize that byte to 00, as Linux's
       shifted raw CID does, instead of treating CRC as medium identity. */
    u32 any=0;
    for(u32 i=0;i<15;++i) { cid[i]=LIBRARY_CARD_TARGET_BYTE(RT_CARD_CID+i); any|=cid[i]; }
    if(!any || LIBRARY_CARD_TARGET_BYTE(RT_CARD_CID+15U)!=1) return 0;
    cid[15]=0;
    return library_card_target_current(target);
}
static int library_card_target_sector(void *context,u32 lba,u8 bytes[512])
{
    struct library_card_target *target=context;
    if(!library_card_target_current(target) || lba>=target->capacity || ((__UINTPTR_TYPE__)bytes&63U)) return 0;
    /* SdDriver::read(driver,lba,count,buffer) returns 1 on success. The
       method delegates to 530a9b30 (III) / 530bfd40 (IV), which acquire
       and release the firmware SD transaction lock around the transfer. */
    return LIBRARY_CARD_TARGET_READ(target->driver,lba,1,bytes)==1 && library_card_target_current(target);
}
static int library_card_target_read_evidence(u8 out[532],struct library_card_scratch *scratch)
{
    struct library_card_target target={0,0};
    if(!library_card_target_current(&target)) return 0;
    struct library_card_reader reader={&target,target.capacity,library_card_target_cid,library_card_target_sector};
    return library_card_read_evidence(&reader,out,scratch);
}
#else
static int library_card_target_read_evidence(u8 out[532],struct library_card_scratch *scratch)
{ (void)out; (void)scratch; return 0; }
#endif
#endif
