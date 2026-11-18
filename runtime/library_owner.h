#ifndef GRKIT_LIBRARY_OWNER_H
#define GRKIT_LIBRARY_OWNER_H
/* The resident loader owns file I/O. The App task may submit a bounded immutable
   candidate, but must not call poll. Firmware calls below use the verified RTOS
   file-task signatures from fastboot/backend.c. No Linux file API is present. */
#include "library_store.h"
#ifndef LIBRARY_OWNER_BARRIER
#define LIBRARY_OWNER_BARRIER() __asm__ volatile("dmb ish" ::: "memory")
#endif
#define LIBRARY_OWNER_NOT_RESIDENT 64
#define LIBRARY_OWNER_UNVERIFIED_DURABILITY 65
#define LIBRARY_OWNER_IDLE 0U
#define LIBRARY_OWNER_QUEUED 1U
#define LIBRARY_OWNER_RUNNING 2U
#define LIBRARY_OWNER_DONE 3U
#define LIBRARY_OWNER_RECOVER 1U
#define LIBRARY_OWNER_COMMIT 2U
#define LIBRARY_OWNER_SEED 3U

struct library_owner_fs {
    int (*open)(const char *,u32,u32);
    int (*close)(u32);
    int (*read)(u32,void *,u32,u32 *);
    int (*write)(u32,void *,u32,u32 *);
    int (*stat)(const char *,void *);
    /* Optional only for read-only recovery. A target must provide this only
       after its close/flush/card completion contract has been established.
       Closing a handle alone must not be substituted for durable(). */
    int (*durable)(void *);
    /* Target admission runs before the first mutation. It can restrict a
       verified close contract to the observed filesystem and block driver. */
    int (*admit_write)(void *);
};
struct library_owner {
    struct library_owner_fs fs;
    void *context;
    int (*is_resident)(void *);
    int (*contents_ready)(void *,const u8 *,u32);
    volatile u32 state;
    u32 operation,length,expected_generation;
    int status;
    struct library_store_view view;
    int handles[2];
    u32 positions[2];
    u8 candidate[LIBRARY_STORE_MANIFEST_MAX];
    u8 scratch[LIBRARY_STORE_MANIFEST_MAX];
    /* One card block: the card reads a whole block for each smaller read,
       so skipping with smaller reads reads each block several times. */
    u8 discard[512];
};
static const char *library_owner_path(u32 slot)
{ return slot ? "C:\\GRKIT\\LIB1.BIN" : "C:\\GRKIT\\LIB0.BIN"; }

/* RT_FS_SEEK is not established by the current target tables. Reading a bounded
   prefix on the same read/write descriptor advances its position without an
   invented firmware address. The resident task pays this cost, never App UI. */
static int library_owner_close_slot(struct library_owner *owner,u32 slot)
{
    if(owner->handles[slot]<0) return 0;
    u32 handle=(u32)owner->handles[slot];
    owner->handles[slot]=-1; owner->positions[slot]=0;
    return owner->fs.close(handle);
}
static int library_owner_close_all(struct library_owner *owner)
{
    int first=library_owner_close_slot(owner,0);
    int second=library_owner_close_slot(owner,1);
    return first || second;
}
static int library_owner_position(struct library_owner *owner,u32 slot,u32 offset)
{
    while(owner->positions[slot]<offset) {
        u32 left=offset-owner->positions[slot];
        u32 count=left<sizeof(owner->discard)?left:(u32)sizeof(owner->discard),read=0;
        if(owner->fs.read((u32)owner->handles[slot],owner->discard,count,&read) || read!=count) return 1;
        owner->positions[slot]+=count;
    }
    return 0;
}
static int library_owner_file(struct library_owner *owner,u32 slot,u32 offset,u8 *bytes,u32 count,int writing)
{
    if(slot>1 || offset>LIBRARY_STORE_SLOT_BYTES || count>LIBRARY_STORE_SLOT_BYTES-offset || !bytes) return 1;
    if(owner->handles[slot]>=0 && offset<owner->positions[slot] && library_owner_close_slot(owner,slot)) return 1;
    if(owner->handles[slot]<0) {
        u32 stat[16]={0};
        const char *path=library_owner_path(slot);
        if(owner->fs.stat(path,stat) || stat[2]!=LIBRARY_STORE_SLOT_BYTES) return 1;
        /* MODE_RDWR=0 preserves the preallocated file. Never truncate/create. */
        int handle=owner->fs.open(path,0,0);
        if(handle<0) return 1;
        owner->handles[slot]=handle; owner->positions[slot]=0;
    }
    u32 transferred=0;
    int failed=library_owner_position(owner,slot,offset);
    if(!failed) {
        u32 handle=(u32)owner->handles[slot];
        failed=writing ? owner->fs.write(handle,bytes,count,&transferred)
                       : owner->fs.read(handle,bytes,count,&transferred);
        if(transferred!=count) failed=1;
        else owner->positions[slot]+=count;
    }
    if(failed) { library_owner_close_slot(owner,slot); return 1; }
    return 0;
}
static int library_owner_read(void *context,u32 slot,u32 offset,u8 *bytes,u32 count)
{ return library_owner_file(context,slot,offset,bytes,count,0); }
static int library_owner_write(void *context,u32 slot,u32 offset,const u8 *bytes,u32 count)
{ return library_owner_file(context,slot,offset,(u8 *)bytes,count,1); }
static int library_owner_sync(void *context,u32 slot)
{
    struct library_owner *owner=context;
    (void)slot;
    return library_owner_close_all(owner) || !owner->fs.durable || owner->fs.durable(owner->context);
}
static int library_owner_contents(void *context,const u8 *bytes,u32 count)
{
    struct library_owner *owner=context;
    return owner->contents_ready(owner->context,bytes,count);
}

static int library_owner_init(struct library_owner *owner,const struct library_owner_fs *fs,void *context,
                              int (*is_resident)(void *),int (*contents_ready)(void *,const u8 *,u32))
{
    if(!owner || !fs || !fs->open || !fs->close || !fs->read || !fs->write || !fs->stat || !is_resident || !contents_ready) return 0;
    owner->fs=*fs; owner->context=context; owner->is_resident=is_resident; owner->contents_ready=contents_ready;
    owner->handles[0]=owner->handles[1]=-1; owner->positions[0]=owner->positions[1]=0;
    owner->state=LIBRARY_OWNER_IDLE; owner->operation=0; owner->length=0; owner->expected_generation=0;
    owner->status=LIBRARY_STORE_OK;
    owner->view.slot=0; owner->view.generation=0; owner->view.length=0; owner->view.checksum=0;
    return 1;
}

/* Single producer, single resident consumer. A queued/running/result candidate
   cannot be replaced. The producer releases a result before submitting another.
   The containing mailbox/session protocol must authenticate its source and
   handle card identity changes before calling this internal scheduler. */
static int library_owner_submit(struct library_owner *owner,u32 operation,u32 expected,const u8 *manifest,u32 length)
{
    if(!owner || owner->state!=LIBRARY_OWNER_IDLE || operation<LIBRARY_OWNER_RECOVER || operation>LIBRARY_OWNER_SEED) return 0;
    LIBRARY_OWNER_BARRIER();
    if(operation==LIBRARY_OWNER_RECOVER) { if(length) return 0; }
    else if(!library_store_manifest(manifest,length)) return 0;
    for(u32 i=0;i<length;++i) owner->candidate[i]=manifest[i];
    owner->operation=operation; owner->expected_generation=expected; owner->length=length;
    LIBRARY_OWNER_BARRIER();
    owner->state=LIBRARY_OWNER_QUEUED;
    return 1;
}
static int library_owner_poll(struct library_owner *owner)
{
    if(!owner || !owner->is_resident(owner->context)) return LIBRARY_OWNER_NOT_RESIDENT;
    if(owner->state!=LIBRARY_OWNER_QUEUED) return LIBRARY_STORE_OK;
    LIBRARY_OWNER_BARRIER();
    owner->state=LIBRARY_OWNER_RUNNING;
    struct library_store_io io={owner,library_owner_read,library_owner_write,library_owner_sync,library_owner_contents};
    int status;
    struct library_store_view result;
    if(owner->operation!=LIBRARY_OWNER_RECOVER && (!owner->fs.durable ||
       (owner->fs.admit_write && owner->fs.admit_write(owner->context)))) status=LIBRARY_OWNER_UNVERIFIED_DURABILITY;
    else if(owner->operation==LIBRARY_OWNER_RECOVER)
        status=library_store_recover(&io,owner->scratch,sizeof(owner->scratch),&result);
    else if(owner->operation==LIBRARY_OWNER_COMMIT)
        status=library_store_commit(&io,owner->expected_generation,owner->candidate,owner->length,owner->scratch,sizeof(owner->scratch),&result);
    else if(owner->operation==LIBRARY_OWNER_SEED)
        status=library_store_seed(&io,owner->candidate,owner->length,owner->scratch,sizeof(owner->scratch),&result);
    else status=LIBRARY_STORE_INVALID;
    /* No descriptor survives a resident poll, including failed readback. */
    if(library_owner_close_all(owner) && !status) status=LIBRARY_STORE_IO;
    if(!status) owner->view=result;
    owner->status=status;
    LIBRARY_OWNER_BARRIER();
    owner->state=LIBRARY_OWNER_DONE;
    return status;
}
static int library_owner_result(struct library_owner *owner,int *status,struct library_store_view *view)
{
    if(!owner || !status || !view || owner->state!=LIBRARY_OWNER_DONE) return 0;
    LIBRARY_OWNER_BARRIER();
    *status=owner->status; *view=owner->view;
    LIBRARY_OWNER_BARRIER();
    owner->state=LIBRARY_OWNER_IDLE;
    return 1;
}
#endif
