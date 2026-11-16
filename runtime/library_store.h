#ifndef GRKIT_LIBRARY_STORE_H
#define GRKIT_LIBRARY_STORE_H
/* RTOS-owned manifest storage. The includer defines u8/u32. The owner provides
   two preallocated 32 KiB files and serializes all calls with runtime commits.
   No callback may route writes through Linux. There are no rename/unlink calls.
   A successful sync must make preceding writes durable on the card, not merely
   visible in an OS cache. CRC detects corruption; it is not authentication. */
#define LIBRARY_STORE_SLOT_BYTES 32768U
#define LIBRARY_STORE_HEADER_BYTES 32U
#define LIBRARY_STORE_MANIFEST_MAX 24736U
#define LIBRARY_STORE_COMMIT_OFFSET (LIBRARY_STORE_SLOT_BYTES-16U)
#define LIBRARY_STORE_OK 0
#define LIBRARY_STORE_INVALID 1
#define LIBRARY_STORE_IO 2
#define LIBRARY_STORE_RECOVERY_REQUIRED 3
#define LIBRARY_STORE_STALE 4
#define LIBRARY_STORE_GENERATION_EXHAUSTED 5
#define LIBRARY_STORE_CONTENT_NOT_READY 6
#define LIBRARY_STORE_NOT_EMPTY 7

struct library_store_io {
    void *owner;
    /* Return zero only when every requested byte was read/written. A failed
       write may have persisted any prefix. Offsets never exceed one slot. */
    int (*read_at)(void *,u32,u32,u8 *,u32);
    int (*write_at)(void *,u32,u32,const u8 *,u32);
    int (*sync)(void *,u32);
    /* Validate full manifest semantics and every referenced immutable content
       generation. Return zero only if those bytes and directory entries are
       durable through the RTOS owner. Removed entries need no live content. */
    int (*contents_ready)(void *,const u8 *,u32);
};
struct library_store_view { u32 slot,generation,length,checksum; };

static u32 library_store_word(const u8 *p)
{ return (u32)p[0]|((u32)p[1]<<8)|((u32)p[2]<<16)|((u32)p[3]<<24); }
static void library_store_put(u8 *p,u32 v)
{ for(u32 i=0;i<4;++i) p[i]=(u8)(v>>(8*i)); }
static int library_store_equal(const u8 *a,const u8 *b,u32 n)
{ for(u32 i=0;i<n;++i) if(a[i]!=b[i]) return 0; return 1; }
static u32 library_store_crc(const u8 *p,u32 n,u32 zero,u32 count)
{
    u32 crc=0xffffffffU;
    for(u32 i=0;i<n;++i) {
        crc^=(i>=zero && i-zero<count) ? 0U : p[i];
        for(u32 b=0;b<8;++b) crc=(crc>>1)^(0xedb88320U&(0U-(crc&1U)));
    }
    return ~crc;
}
/* The Rust library codec owns this wire format: GRKLIB01, format 1, 32-byte
   header, 96-byte ID-sorted entries, and u16 order indices. This checks framing
   and integrity. contents_ready must additionally check the entry semantics. */
static int library_store_manifest(const u8 *p,u32 n)
{
    static const u8 magic[8]={'G','R','K','L','I','B','0','1'};
    if(!p || n<32 || n>LIBRARY_STORE_MANIFEST_MAX || !library_store_equal(p,magic,8)) return 0;
    u32 entries=library_store_word(p+16),order=library_store_word(p+20);
    return library_store_word(p+8)==1 && library_store_word(p+12)==n && entries<=256 && order<=64 &&
        n==32+96*entries+2*order && !library_store_word(p+28) &&
        library_store_word(p+24)==library_store_crc(p,n,24,4);
}
static int library_store_callbacks(const struct library_store_io *io)
{ return io && io->read_at && io->write_at && io->sync && io->contents_ready; }

static int library_store_read_slot(const struct library_store_io *io,u32 slot,u8 *scratch,struct library_store_view *view)
{
    static const u8 magic[8]={'G','R','K','S','L','O','T','1'};
    u8 header[32],commit[16];
    if(io->read_at(io->owner,slot,0,header,sizeof(header)) ||
       io->read_at(io->owner,slot,LIBRARY_STORE_COMMIT_OFFSET,commit,sizeof(commit))) return LIBRARY_STORE_IO;
    u32 generation=library_store_word(header+8),length=library_store_word(header+12),crc=library_store_word(header+16);
    if(!library_store_equal(header,magic,8) || !generation || length<32 || length>LIBRARY_STORE_MANIFEST_MAX ||
       library_store_word(header+24)!=1 || library_store_word(header+28) ||
       library_store_word(header+20)!=library_store_crc(header,32,20,4) ||
       library_store_word(commit)!=0x54494d43U || library_store_word(commit+4)!=generation ||
       library_store_word(commit+8)!=crc || library_store_word(commit+12)!=library_store_crc(commit,16,12,4))
        return LIBRARY_STORE_INVALID;
    if(io->read_at(io->owner,slot,32,scratch,length)) return LIBRARY_STORE_IO;
    if(library_store_crc(scratch,length,0,0)!=crc || !library_store_manifest(scratch,length)) return LIBRARY_STORE_INVALID;
    view->slot=slot; view->generation=generation; view->length=length; view->checksum=crc;
    return LIBRARY_STORE_OK;
}

/* scratch has at least MANIFEST_MAX bytes. On success it contains the chosen
   manifest and view names its durable generation. Neither result is published
   to camera readers here. An unreadable slot cannot silently roll back a newer
   generation; only a fully read but invalid slot permits fallback. */
static int library_store_recover(const struct library_store_io *io,u8 *scratch,u32 capacity,struct library_store_view *out)
{
    if(!library_store_callbacks(io) || !scratch || capacity<LIBRARY_STORE_MANIFEST_MAX || !out) return LIBRARY_STORE_INVALID;
    struct library_store_view views[2],verify;
    int first=library_store_read_slot(io,0,scratch,&views[0]);
    int second=library_store_read_slot(io,1,scratch,&views[1]);
    if(first==LIBRARY_STORE_IO || second==LIBRARY_STORE_IO) return LIBRARY_STORE_IO;
    if(first && second) return LIBRARY_STORE_RECOVERY_REQUIRED;
    if(!first && !second && views[0].generation==views[1].generation) return LIBRARY_STORE_RECOVERY_REQUIRED;
    u32 chosen=first ? 1U : second ? 0U : views[1].generation>views[0].generation;
    int status=library_store_read_slot(io,chosen,scratch,&verify);
    if(status) return status==LIBRARY_STORE_IO ? status : LIBRARY_STORE_RECOVERY_REQUIRED;
    if(verify.generation!=views[chosen].generation || verify.length!=views[chosen].length || verify.checksum!=views[chosen].checksum)
        return LIBRARY_STORE_RECOVERY_REQUIRED;
    if(io->contents_ready(io->owner,scratch,verify.length)) return LIBRARY_STORE_RECOVERY_REQUIRED;
    *out=verify;
    return LIBRARY_STORE_OK;
}

/* Every failure after writing starts requires recovery before another mutation.
   In particular, a failed final sync may already have committed the new slot. */
static int library_store_write(const struct library_store_io *io,u32 slot,u32 generation,const u8 *manifest,u32 length,
                               u8 *scratch,u32 capacity,struct library_store_view *out)
{
    u8 header[32]={'G','R','K','S','L','O','T','1'},commit[16]={0};
    u32 crc=library_store_crc(manifest,length,0,0);
    library_store_put(header+8,generation); library_store_put(header+12,length); library_store_put(header+16,crc);
    library_store_put(header+24,1); library_store_put(header+20,library_store_crc(header,32,20,4));
    if(io->write_at(io->owner,slot,LIBRARY_STORE_COMMIT_OFFSET,commit,16) || io->sync(io->owner,slot)) return LIBRARY_STORE_IO;
    if(io->write_at(io->owner,slot,0,header,32) || io->write_at(io->owner,slot,32,manifest,length) ||
       io->sync(io->owner,slot)) return LIBRARY_STORE_IO;
    library_store_put(commit,0x54494d43U); library_store_put(commit+4,generation); library_store_put(commit+8,crc);
    library_store_put(commit+12,library_store_crc(commit,16,12,4));
    if(io->write_at(io->owner,slot,LIBRARY_STORE_COMMIT_OFFSET,commit,16) || io->sync(io->owner,slot)) return LIBRARY_STORE_IO;
    struct library_store_view verified;
    int status=library_store_recover(io,scratch,capacity,&verified);
    if(status) return status;
    if(verified.slot!=slot || verified.generation!=generation || verified.length!=length || verified.checksum!=crc)
        return LIBRARY_STORE_RECOVERY_REQUIRED;
    *out=verified;
    return LIBRARY_STORE_OK;
}

/* manifest and scratch must not overlap: recovery replaces scratch. The owner
   must retain the immutable candidate until readback completes. */
static int library_store_commit(const struct library_store_io *io,u32 expected_generation,const u8 *manifest,u32 length,
                                u8 *scratch,u32 capacity,struct library_store_view *out)
{
    if(!library_store_callbacks(io) || !scratch || capacity<LIBRARY_STORE_MANIFEST_MAX || !out || !library_store_manifest(manifest,length))
        return LIBRARY_STORE_INVALID;
    struct library_store_view current;
    int status=library_store_recover(io,scratch,capacity,&current);
    if(status) return status;
    if(current.generation!=expected_generation) return LIBRARY_STORE_STALE;
    if(current.generation==0xffffffffU) return LIBRARY_STORE_GENERATION_EXHAUSTED;
    if(io->contents_ready(io->owner,manifest,length)) return LIBRARY_STORE_CONTENT_NOT_READY;
    return library_store_write(io,1U-current.slot,current.generation+1U,manifest,length,scratch,capacity,out);
}

/* Initial provisioning is explicit and separate from commit. Both slots must
   already exist at their fixed size and contain only zeros. If provisioning is
   interrupted, the owner must retain the previous unmanaged library and recover
   the format transition; normal recovery never treats two bad slots as empty. */
static int library_store_seed(const struct library_store_io *io,const u8 *manifest,u32 length,
                              u8 *scratch,u32 capacity,struct library_store_view *out)
{
    if(!library_store_callbacks(io) || !scratch || capacity<LIBRARY_STORE_MANIFEST_MAX || !out || !library_store_manifest(manifest,length))
        return LIBRARY_STORE_INVALID;
    for(u32 slot=0;slot<2;++slot) for(u32 offset=0;offset<LIBRARY_STORE_SLOT_BYTES;offset+=256) {
        if(io->read_at(io->owner,slot,offset,scratch,256)) return LIBRARY_STORE_IO;
        for(u32 i=0;i<256;++i) if(scratch[i]) return LIBRARY_STORE_NOT_EMPTY;
    }
    if(io->contents_ready(io->owner,manifest,length)) return LIBRARY_STORE_CONTENT_NOT_READY;
    return library_store_write(io,0,1,manifest,length,scratch,capacity,out);
}
#endif
