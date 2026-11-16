#ifndef GRKIT_LIBRARY_DIRECTORY_TARGET_H
#define GRKIT_LIBRARY_DIRECTORY_TARGET_H
/* Native exFAT maps EOF and several unrelated errors to the same public -22.
   Normalize only the verified directory cursor's terminal state. */
#define LIBRARY_DIRECTORY_BYTES 0x330U
#ifndef LIBRARY_DIRECTORY_WORD
#define LIBRARY_DIRECTORY_WORD(address) (*(volatile u32 *)(__UINTPTR_TYPE__)(address))
#endif
#ifndef LIBRARY_DIRECTORY_NATIVE_READ
#define LIBRARY_DIRECTORY_NATIVE_READ(handle,out) (((int (*)(u32,void *))(__UINTPTR_TYPE__)RT_FS_READDIR)(handle,out))
#endif
#if RT_FS_CLOSE == 0x53845930
#define LIBRARY_DIRECTORY_ROOT 0x55349e1cU
#define LIBRARY_DIRECTORY_FS 0x53d2d200U
#define LIBRARY_DIRECTORY_FAT 0x53d2ce64U
#define LIBRARY_DIRECTORY_EXFAT 0x53d29f4cU
#define LIBRARY_DIRECTORY_PRIMARY 0x53d29ea8U
#define LIBRARY_DIRECTORY_VERIFIED 1
#elif RT_FS_CLOSE == 0x53ac3d80
#define LIBRARY_DIRECTORY_ROOT 0x5537fc08U
#define LIBRARY_DIRECTORY_FS 0x53fd39f0U
#define LIBRARY_DIRECTORY_FAT 0x53fd3654U
#define LIBRARY_DIRECTORY_EXFAT 0x53fd06a4U
#define LIBRARY_DIRECTORY_PRIMARY 0x53fd0600U
#define LIBRARY_DIRECTORY_VERIFIED 1
#endif
#ifdef LIBRARY_DIRECTORY_VERIFIED
struct library_directory_context { u32 fs,drives,instance,raw,drive,state,exfat; };
static int library_directory_pointer(u32 address,u32 size)
{ return !(address&3U) && address>=0x53000000U && size<=0x1d000000U && address<=0x70000000U-size; }
static int library_directory_context(u32 handle,struct library_directory_context *context)
{
    if(!library_directory_pointer(handle,8)) return 0;
    context->fs=LIBRARY_DIRECTORY_WORD(LIBRARY_DIRECTORY_ROOT);
    if(!library_directory_pointer(context->fs,0x1c) || LIBRARY_DIRECTORY_WORD(context->fs)!=LIBRARY_DIRECTORY_FS) return 0;
    context->drives=LIBRARY_DIRECTORY_WORD(context->fs+0x18);
    if(!library_directory_pointer(context->drives,4)) return 0;
    context->drive=LIBRARY_DIRECTORY_WORD(handle+4);
    /* These entry points enumerate only the mounted C: slot. */
    if(context->drive) return 0;
    context->instance=LIBRARY_DIRECTORY_WORD(context->drives);
    if(!library_directory_pointer(context->instance,4)) return 0;
    u32 table=LIBRARY_DIRECTORY_WORD(context->instance);
    context->exfat=table==LIBRARY_DIRECTORY_EXFAT;
    if(!context->exfat && table!=LIBRARY_DIRECTORY_FAT) return 0;
    if(context->exfat && (context->instance<0x53000004U ||
       LIBRARY_DIRECTORY_WORD(context->instance-4)!=LIBRARY_DIRECTORY_PRIMARY)) return 0;
    context->raw=LIBRARY_DIRECTORY_WORD(handle);
    if(!library_directory_pointer(context->raw,context->exfat ? 0x24cU : 4U)) return 0;
    if(context->exfat && LIBRARY_DIRECTORY_WORD(context->raw+0x11c)!=context->drive) return 0;
    context->state=context->exfat ? LIBRARY_DIRECTORY_WORD(context->raw+0x140) : 0;
    return 1;
}
static int library_directory_same(const struct library_directory_context *a,const struct library_directory_context *b)
{ return a->fs==b->fs && a->drives==b->drives && a->instance==b->instance && a->raw==b->raw && a->drive==b->drive && a->exfat==b->exfat; }
#endif
static int library_directory_target_read(u32 handle,void *out)
{
#ifdef LIBRARY_DIRECTORY_VERIFIED
    struct library_directory_context before,after;
    if(!out || !library_directory_context(handle,&before)) return -5;
    if(before.exfat && before.state!=0 && before.state!=0xfffffffeU) return -5;
    int result=LIBRARY_DIRECTORY_NATIVE_READ(handle,out);
    if(!library_directory_context(handle,&after) || !library_directory_same(&before,&after)) return -5;
    if(!before.exfat) return result;
    if(after.state!=0 && after.state!=0xfffffffeU) return -5;
    if(before.state==0xfffffffeU) {
        if(result!=-22 || after.state!=before.state) return -5;
        for(u32 i=0;i<LIBRARY_DIRECTORY_BYTES;++i) if(((const u8 *)out)[i]) return -5;
        return 1;
    }
    return result==0 ? 0 : -5;
#else
    return LIBRARY_DIRECTORY_NATIVE_READ(handle,out);
#endif
}
#endif
