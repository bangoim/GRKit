#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
#include "library_apply_stream.h"
#include "grf.h"
static u8 bundle[LIBRARY_APPLY_MAX+1],manifest[LIBRARY_STORE_MANIFEST_MAX],content[GRKIT_GRF_MAX_SIZE];
static u8 header[128],frame[GRKIT_CONTROL_PAYLOAD_BYTES],ack[128],card_hash[32]={1};
static u32 epoch[4]={1,2,3,4},size;
static struct library_apply_stream stream;
static struct library_apply_buffers buffers={manifest,content,sizeof(manifest),sizeof(content)};
static int validation_fails;
static int validate(const u8 *p,u32 n) { return !validation_fails && grkit_grf_validate(p,n); }
static void make_header(void)
{
    memset(header,0,sizeof(header)); memcpy(header,"GRS1",4); header[4]=1;
    for(u32 i=0;i<4;++i) library_store_put(header+20+4*i,epoch[i]);
    memcpy(header+36,card_hash,32); library_store_put(header+68,7); library_store_put(header+72,size);
    struct library_sha256_state hash; library_sha256_init(&hash);
    assert(library_sha256_update(&hash,bundle,size)); assert(library_sha256_final(&hash,header+80));
}
static u32 make_frame(u32 offset,u32 count)
{
    memcpy(frame,header,128); library_store_put(frame+76,offset); memcpy(frame+128,bundle+offset,count); return 128+count;
}
static int receive(u32 chunk)
{
    memset(&stream,0,sizeof(stream));
    for(u32 offset=0;offset<size;) {
        u32 count=size-offset; if(count>chunk) count=chunk;
        u32 length=make_frame(offset,count);
        if(!offset && !library_apply_admit(&stream,frame,length,epoch,card_hash,7,bundle,1)) return 0;
        if(!library_apply_write(&stream,&buffers,frame,length,2,validate,ack)) return 0;
        offset+=count;
        assert(library_store_word(ack+76)==offset);
        assert(!library_apply_write(&stream,&buffers,frame,length,2,validate,ack));
    }
    assert(stream.ready && !stream.sealed);
    return 1;
}
static void boundaries(void)
{
    u32 length;
    memset(&stream,0,sizeof(stream)); length=make_frame(0,1);
    assert(library_apply_admit(&stream,frame,length,epoch,card_hash,7,bundle,1));
    memset(ack,0x5a,sizeof(ack));
    assert(!library_apply_write(&stream,&buffers,frame,length,2,validate,manifest));
    assert(!library_apply_write(&stream,&buffers,frame,length,2,validate,content));
    assert(!library_apply_write(&stream,&buffers,frame,length,2,validate,(u8 *)&stream));
    assert(!library_apply_write(&stream,&buffers,frame,length,2,validate,(u8 *)&buffers));
    assert(stream.received==0 && !stream.ready);
    for(u32 i=0;i<128;++i) assert(ack[i]==0x5a);
    struct library_apply_buffers wrong=buffers;
    wrong.content=wrong.manifest;
    assert(!library_apply_write(&stream,&wrong,frame,length,2,validate,ack));
    wrong=buffers; wrong.manifest=frame;
    assert(!library_apply_write(&stream,&wrong,frame,length,2,validate,ack));
    /* Reusing the consumed input for ACK is valid and does not alter staging. */
    assert(library_apply_write(&stream,&buffers,frame,length,2,validate,frame));
    assert(stream.received==1);
    length=make_frame(1,1); wrong=buffers; --wrong.manifest_capacity;
    assert(!library_apply_write(&stream,&wrong,frame,length,2,validate,ack));
    assert(stream.received==1);
    wrong=buffers; wrong.content=content+1; --wrong.content_capacity;
    assert(!library_apply_write(&stream,&wrong,frame,length,2,validate,ack));
    assert(stream.received==1);
    assert(library_apply_write(&stream,&buffers,frame,length,2,validate,ack));
    assert(library_apply_abort(&stream,ack,128,3) && !stream.ready);
    /* A manifest length crossing frame boundaries must fail before storage. */
    memset(&stream,0,sizeof(stream)); length=make_frame(0,35);
    assert(library_apply_admit(&stream,frame,length,epoch,card_hash,7,bundle,1));
    wrong=buffers; wrong.manifest_capacity=1;
    assert(library_apply_write(&stream,&wrong,frame,length,2,validate,ack));
    length=make_frame(35,1); assert(!library_apply_write(&stream,&wrong,frame,length,2,validate,ack));
    assert(!stream.active && !stream.ready);
    /* Expiry clears an unsealed ready candidate, but never a sealed commit. */
    assert(receive(8576));
    library_apply_expire(&stream,LIBRARY_APPLY_TTL_MS+2);
    assert(!stream.active && !stream.ready);
    stream.active=1; stream.ready=1; stream.sealed=0; stream.started=0xfffffff0U;
    library_apply_expire(&stream,16); assert(stream.active);
    library_apply_expire(&stream,LIBRARY_APPLY_TTL_MS); assert(!stream.active && !stream.ready);
}
int main(int argc,char **argv)
{
    assert(argc==3); FILE *file=fopen(argv[1],"rb"); assert(file);
    size=(u32)fread(bundle,1,sizeof(bundle),file); assert(!ferror(file) && size<=LIBRARY_APPLY_MAX); fclose(file);
    u32 chunk=(u32)strtoul(argv[2],0,10); assert(chunk && chunk<=GRKIT_CONTROL_PAYLOAD_BYTES-128);
    make_header(); boundaries();
    u32 manifest_size=library_store_word(bundle+32),content_size=library_store_word(bundle+36+manifest_size);
    if(content_size) {
        validation_fails=1; assert(!receive(chunk));
        assert(!stream.active && !stream.ready); validation_fails=0;
    }
    u8 last=bundle[size-4]; bundle[size-4]=2; make_header();
    assert(!receive(chunk)); assert(!stream.active && !stream.ready);
    bundle[size-4]=last; make_header();
    assert(receive(chunk));
    assert(library_apply_seal(&stream,ack,128,3));
    library_apply_expire(&stream,100000); assert(stream.active && stream.sealed);
    assert(!library_apply_abort(&stream,ack,128,100001));
    u8 result[12]; library_store_put(result,stream.manifest_length); library_store_put(result+4,stream.content_length); library_store_put(result+8,stream.select_off);
    assert(fwrite(result,1,12,stdout)==12);
    assert(fwrite(manifest,1,stream.manifest_length,stdout)==stream.manifest_length);
    assert(fwrite(content,1,stream.content_length,stdout)==stream.content_length);
    memset(&stream,0,sizeof(stream)); u32 length=make_frame(0,1);
    assert(library_apply_admit(&stream,frame,length,epoch,card_hash,7,bundle,1));
    assert(library_apply_write(&stream,&buffers,frame,length,2,validate,ack));
    assert(!library_apply_seal(&stream,ack,128,3));
    assert(library_apply_abort(&stream,ack,128,3));
    assert(library_apply_admit(&stream,frame,length,epoch,card_hash,7,bundle,1));
    library_apply_expire(&stream,LIBRARY_APPLY_TTL_MS+2); assert(!stream.active);
    frame[68]^=1; assert(!library_apply_admit(&stream,frame,length,epoch,card_hash,7,bundle,1)); frame[68]^=1;
    header[80]^=1; assert(!receive(chunk)); header[80]^=1;
    bundle[36+24]^=1; make_header(); assert(!receive(chunk)); bundle[36+24]^=1; make_header();
    memset(&stream,0,sizeof(stream)); length=make_frame(0,1);
    u8 wrong[32]; memcpy(wrong,bundle,32); wrong[0]^=1;
    assert(library_apply_admit(&stream,frame,length,epoch,card_hash,7,wrong,1));
    assert(!library_apply_write(&stream,&buffers,frame,length,2,validate,ack));
    return 0;
}
