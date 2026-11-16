#ifndef GRKIT_LIBRARY_OWNER_TARGET_H
#define GRKIT_LIBRARY_OWNER_TARGET_H
/* Include the exact fastboot/targets/<camera>-<version>.h selected by the loader
   build before this header. These addresses are already used by the RTOS-native
   installer. This adapter must execute in the resident loader task. */
#include "library_owner.h"
#if !defined(RT_FS_OPEN) || !defined(RT_FS_CLOSE) || !defined(RT_FS_READ) || !defined(RT_FS_WRITE) || !defined(RT_FS_STAT_SIZE)
#error "Select the verified RTOS file API target before library_owner_target.h"
#endif
#if RT_FS_CLOSE == 0x53845930 || RT_FS_CLOSE == 0x53ac3d80
static int library_owner_target_pointer(u32 address)
{ return address>=0x53000000U && address<0x70000000U && !(address&3U); }
#ifndef LIBRARY_OWNER_TARGET_WORD
#define LIBRARY_OWNER_TARGET_WORD(address) (*(volatile u32 *)(__UINTPTR_TYPE__)(address))
#endif
static u32 library_owner_target_word(u32 address)
{ return LIBRARY_OWNER_TARGET_WORD(address); }
#endif
#if RT_FS_CLOSE == 0x53845930 || RT_FS_CLOSE == 0x53ac3d80
#include "library_card_target.h"
#if RT_FS_CLOSE == 0x53ac3d80
#include "library_exfat_target.h"
#endif
#if RT_FS_CLOSE == 0x53845930
#define LIBRARY_OWNER_FILESYSTEM_ROOT 0x55349e1cU
#define LIBRARY_OWNER_FILESYSTEM_VTABLE 0x53d2d200U
#define LIBRARY_OWNER_FAT_VTABLE 0x53d2ce64U
#else
#define LIBRARY_OWNER_FILESYSTEM_ROOT 0x5537fc08U
#define LIBRARY_OWNER_FILESYSTEM_VTABLE 0x53fd39f0U
#define LIBRARY_OWNER_FAT_VTABLE 0x53fd3654U
#endif
/* The C: filesystem slot and its FAT implementation are checked separately
   from the physical card driver. An exFAT close can hide a flush error, so it
   must not inherit the FAT barrier merely because the SD driver is verified. */
static int library_owner_target_admit(void *context)
{
    (void)context;
    u32 filesystem=library_owner_target_word(LIBRARY_OWNER_FILESYSTEM_ROOT);
    if(!library_owner_target_pointer(filesystem) || library_owner_target_word(filesystem)!=LIBRARY_OWNER_FILESYSTEM_VTABLE) return 1;
    u32 drives=library_owner_target_word(filesystem+0x18U);
    if(!library_owner_target_pointer(drives)) return 1;
    u32 fat=library_owner_target_word(drives);
    if(!library_owner_target_pointer(fat)) return 1;
    if(library_owner_target_word(fat)!=LIBRARY_OWNER_FAT_VTABLE) {
#if RT_FS_CLOSE == 0x53ac3d80
        if(!library_exfat_target_probe()) return 1;
#else
        return 1;
#endif
    }
    struct library_card_target card={0,0};
    return !library_card_target_current(&card);
}
static int library_owner_target_completed(void *context)
{
    /* owner_sync closed every descriptor and checked its status. Verified FAT
       close flushes file data, FAT copies, and directory data synchronously.
       GR III: 5388d044 -> 53592898 -> SdDriver 537fe6d4.
       GR IV: 53b0b674 -> 537cb52c -> SdDriver 53a753b0.
       Both wait for the SD command/data completion before returning. Emulator
       close-return and subsequent block-image readback corroborate this chain;
       this does not promise physical flash survival after power loss. */
    if(library_owner_target_admit(context)) return 1;
#if RT_FS_CLOSE == 0x53ac3d80
    u32 filesystem=library_owner_target_word(LIBRARY_OWNER_FILESYSTEM_ROOT);
    if(!library_owner_target_pointer(filesystem) || library_owner_target_word(filesystem)!=LIBRARY_OWNER_FILESYSTEM_VTABLE) return 1;
    u32 drives=library_owner_target_word(filesystem+0x18U);
    if(!library_owner_target_pointer(drives)) return 1;
    u32 instance=library_owner_target_word(drives);
    if(!library_owner_target_pointer(instance)) return 1;
    if(library_owner_target_word(instance)!=LIBRARY_OWNER_FAT_VTABLE) {
        /* Native exFAT close ignores flush errors; propagate the explicit,
           mutex-protected flush and recheck the mounted card afterwards. */
        if(library_exfat_target_flush()) return 1;
        return library_owner_target_admit(context);
    }
#endif
    return 0;
}
#endif
#if RT_FS_CLOSE == 0x53845930
#define LIBRARY_OWNER_CURRENT_TCB 0x557f1054U
#define LIBRARY_OWNER_SCRIPT_VTABLE 0x53d6ac08U
#define LIBRARY_OWNER_SCRIPT_NAME 0x53b26214U
#elif RT_FS_CLOSE == 0x53ac3d80
/* GR IV 1.21: 530861a4 indexes the current TCB by MPIDR; the Script
   object was independently read through TCB.exinf in the simulator. */
#define LIBRARY_OWNER_CURRENT_TCB 0x558269bcU
#define LIBRARY_OWNER_SCRIPT_VTABLE 0x5401dd78U
#define LIBRARY_OWNER_SCRIPT_NAME 0x53da63b4U
#endif
#if defined(LIBRARY_OWNER_CURRENT_TCB)
static int library_owner_target_resident(void *context)
{
    (void)context;
#ifdef __arm__
    u32 saved,core,tcb,valid=0;
    /* Read this core's current TCB with interrupts masked, so task migration
       cannot separate MPIDR from the current-task array lookup. The verified target
       Script object is identified by its pinned vtable and name. Numeric task
       IDs change with boot ordering (observed 0x01000020 on fast boot). App
       and interrupt contexts are never admitted. */
    __asm__ volatile("mrs %0, cpsr\n\tcpsid i\n\tmrc p15, 0, %1, c0, c0, 5"
                     : "=r"(saved),"=r"(core) :: "memory");
    if((saved&31U)==19U && (core&255U)<3U) {
        tcb=library_owner_target_word(LIBRARY_OWNER_CURRENT_TCB+4U*(core&255U));
        if(library_owner_target_pointer(tcb)) {
            u32 object=library_owner_target_word(tcb+12U);
            if(library_owner_target_pointer(object))
                valid=library_owner_target_word(object)==LIBRARY_OWNER_SCRIPT_VTABLE &&
                    library_owner_target_word(object+8U)==LIBRARY_OWNER_SCRIPT_NAME &&
                    library_owner_target_word(object+4U)==library_owner_target_word(tcb+8U);
        }
    }
    __asm__ volatile("msr cpsr_c, %0" :: "r"(saved) : "memory");
    return valid;
#else
    return 0;
#endif
}
#endif
static struct library_owner_fs library_owner_target_fs(void)
{
    struct library_owner_fs fs={
        (int (*)(const char *,u32,u32))(__UINTPTR_TYPE__)RT_FS_OPEN,
        (int (*)(u32))(__UINTPTR_TYPE__)RT_FS_CLOSE,
        (int (*)(u32,void *,u32,u32 *))(__UINTPTR_TYPE__)RT_FS_READ,
        (int (*)(u32,void *,u32,u32 *))(__UINTPTR_TYPE__)RT_FS_WRITE,
        (int (*)(const char *,void *))(__UINTPTR_TYPE__)RT_FS_STAT_SIZE,
        0,0
    };
#if RT_FS_CLOSE == 0x53845930 || RT_FS_CLOSE == 0x53ac3d80
    fs.admit_write=library_owner_target_admit;
    fs.durable=library_owner_target_completed;
#endif
    return fs;
}
#endif
