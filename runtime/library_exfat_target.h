#ifndef GRKIT_LIBRARY_EXFAT_TARGET_H
#define GRKIT_LIBRARY_EXFAT_TARGET_H
/* GR III 2.21 and GR IV 1.21. Native exFAT close discards cache-flush errors. This
   adapter checks the mounted C: volume and propagates an explicit flush.
   Call only from the resident Script task while FileFactory is alive. */
#if RT_FS_CLOSE == 0x53845930
#define LIBRARY_EXFAT_FS_ROOT 0x55349e1cU
#define LIBRARY_EXFAT_FS_VTABLE 0x53d2d200U
#define LIBRARY_EXFAT_PRIMARY_VTABLE 0x53d29ea8U
#define LIBRARY_EXFAT_SECONDARY_VTABLE 0x53d29f4cU
#define LIBRARY_EXFAT_PARTITION_MANAGER 0x55349decU
#define LIBRARY_EXFAT_ENTRY_STRIDE 0x18U
#define LIBRARY_EXFAT_VOLUME_TABLE 0x5537a238U
#define LIBRARY_EXFAT_MUTEX_OFFSET 0x14U
#define LIBRARY_EXFAT_INSTANCE_BYTES 0x28U
#define LIBRARY_EXFAT_LOCK_ADDRESS 0x536b1cf4U
#define LIBRARY_EXFAT_UNLOCK_ADDRESS 0x536b1e48U
#define LIBRARY_EXFAT_FLUSH_ADDRESS 0x5389a3f4U
#elif RT_FS_CLOSE == 0x53ac3d80
#define LIBRARY_EXFAT_FS_ROOT 0x5537fc08U
#define LIBRARY_EXFAT_FS_VTABLE 0x53fd39f0U
#define LIBRARY_EXFAT_PRIMARY_VTABLE 0x53fd0600U
#define LIBRARY_EXFAT_SECONDARY_VTABLE 0x53fd06a4U
#define LIBRARY_EXFAT_PARTITION_MANAGER 0x5537fbd8U
#define LIBRARY_EXFAT_ENTRY_STRIDE 0x1cU
#define LIBRARY_EXFAT_VOLUME_TABLE 0x553af308U
#define LIBRARY_EXFAT_MUTEX_OFFSET 0x18U
#define LIBRARY_EXFAT_INSTANCE_BYTES 0x2cU
#define LIBRARY_EXFAT_LOCK_ADDRESS 0x539085c8U
#define LIBRARY_EXFAT_UNLOCK_ADDRESS 0x5390871cU
#define LIBRARY_EXFAT_FLUSH_ADDRESS 0x53b18a24U
#else
#error "Select a verified exFAT target before library_exfat_target.h"
#endif
#ifndef LIBRARY_EXFAT_WORD
#define LIBRARY_EXFAT_WORD(address) (*(volatile u32 *)(__UINTPTR_TYPE__)(address))
#endif
#ifndef LIBRARY_EXFAT_LOCK
#define LIBRARY_EXFAT_LOCK(mutex) (((u32 (*)(u32,u32))(__UINTPTR_TYPE__)LIBRARY_EXFAT_LOCK_ADDRESS)(mutex,0xffffffffU))
#define LIBRARY_EXFAT_UNLOCK(mutex) (((u32 (*)(u32))(__UINTPTR_TYPE__)LIBRARY_EXFAT_UNLOCK_ADDRESS)(mutex))
#define LIBRARY_EXFAT_FLUSH(volume) (((int (*)(u32))(__UINTPTR_TYPE__)LIBRARY_EXFAT_FLUSH_ADDRESS)(volume))
#endif
struct library_exfat_mount { u32 filesystem,drives,instance,manager,entries,volume; };
static int library_exfat_pointer(u32 address,u32 bytes)
{ return address>=0x53000000U && !(address&3U) && bytes<=0x1d000000U && address<=0x70000000U-bytes; }
static int library_exfat_mount(struct library_exfat_mount *mount)
{
    u32 fs=LIBRARY_EXFAT_WORD(LIBRARY_EXFAT_FS_ROOT);
    if(!library_exfat_pointer(fs,0x1cU) || LIBRARY_EXFAT_WORD(fs)!=LIBRARY_EXFAT_FS_VTABLE) return 0;
    u32 drives=LIBRARY_EXFAT_WORD(fs+0x18U);
    if(!library_exfat_pointer(drives,4)) return 0;
    u32 instance=LIBRARY_EXFAT_WORD(drives);
    /* The secondary interface starts four bytes into the target object.
       Lock/unlock access the embedded mutex through its +0x10 word. */
    if(!library_exfat_pointer(instance,LIBRARY_EXFAT_INSTANCE_BYTES) || instance<0x53000004U ||
       LIBRARY_EXFAT_WORD(instance)!=LIBRARY_EXFAT_SECONDARY_VTABLE ||
       LIBRARY_EXFAT_WORD(instance-4)!=LIBRARY_EXFAT_PRIMARY_VTABLE) return 0;
    u32 manager=LIBRARY_EXFAT_WORD(LIBRARY_EXFAT_PARTITION_MANAGER);
    if(!library_exfat_pointer(manager,0x24U)) return 0;
    u32 entries=LIBRARY_EXFAT_WORD(manager+0x14U),count=LIBRARY_EXFAT_WORD(manager+0x20U);
    if(!count || count>26U || !library_exfat_pointer(entries,count*LIBRARY_EXFAT_ENTRY_STRIDE)) return 0;
    u32 volume=0xffffffffU;
    for(u32 i=0;i<count;++i) {
        if((LIBRARY_EXFAT_WORD(entries+i*LIBRARY_EXFAT_ENTRY_STRIDE+4U)&255U)!='C') continue;
        if(volume!=0xffffffffU) return 0;
        volume=i;
    }
    if(volume>1U) return 0;
    u32 state=LIBRARY_EXFAT_WORD(LIBRARY_EXFAT_VOLUME_TABLE+volume*0x50U);
    if(!(state&255U) || ((state>>8)&255U)!=1U || ((state>>16)&255U)!=volume ||
       LIBRARY_EXFAT_WORD(LIBRARY_EXFAT_VOLUME_TABLE+0x20U+volume*0x50U)!=0) return 0;
    mount->filesystem=fs; mount->drives=drives; mount->instance=instance;
    mount->manager=manager; mount->entries=entries; mount->volume=volume;
    return 1;
}
static int library_exfat_same(const struct library_exfat_mount *a,const struct library_exfat_mount *b)
{
    return a->filesystem==b->filesystem && a->drives==b->drives && a->instance==b->instance &&
        a->manager==b->manager && a->entries==b->entries && a->volume==b->volume;
}
static int library_exfat_target_probe(void)
{ struct library_exfat_mount mount; return library_exfat_mount(&mount); }
static int library_exfat_target_flush(void)
{
    struct library_exfat_mount before,locked,after;
    if(!library_exfat_mount(&before)) return 1;
    /* The secondary interface is primary+4; the target-specific offset locates its mutex.
       Mount, unmount and normal file operations use this same mutex. */
    u32 mutex=before.instance+LIBRARY_EXFAT_MUTEX_OFFSET;
    if(LIBRARY_EXFAT_LOCK(mutex)!=1U) return 1;
    int failed=!library_exfat_mount(&locked) || !library_exfat_same(&before,&locked);
    if(!failed) failed=LIBRARY_EXFAT_FLUSH(locked.volume)!=0;
    if(!failed) failed=!library_exfat_mount(&after) || !library_exfat_same(&locked,&after);
    if(LIBRARY_EXFAT_UNLOCK(mutex)!=1U) failed=1;
    return failed;
}
#endif
