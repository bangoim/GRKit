#ifndef GRKIT_LIBRARY_APPLY_STREAM_H
#define GRKIT_LIBRARY_APPLY_STREAM_H
#include "library_content.h"
#define LIBRARY_APPLY_HEADER 128U
#define LIBRARY_APPLY_MAX (44U+LIBRARY_STORE_MANIFEST_MAX+GRKIT_GRF_MAX_SIZE)
#define LIBRARY_APPLY_TTL_MS 15000U
struct library_apply_stream {
    u8 header[128],before_hash[32];
    struct library_sha256_state hash;
    u32 active,sealed,ready,started,received,manifest_length,content_length,select_off;
    /* The first chunk binds storage for the entire staged candidate. */
    u8 *manifest_buffer,*content_buffer;
    u32 manifest_capacity,content_capacity;
};
struct library_apply_buffers { u8 *manifest,*content; u32 manifest_capacity,content_capacity; };
static int library_apply_overlap(const void *left,u32 left_size,const void *right,u32 right_size)
{
    __UINTPTR_TYPE__ a=(__UINTPTR_TYPE__)left,b=(__UINTPTR_TYPE__)right;
    return left_size && right_size && (a<=b ? b-a<left_size : a-b<right_size);
}
static void library_apply_discard(struct library_apply_stream *stream)
{ stream->active=stream->ready=0; }
static void library_apply_expire(struct library_apply_stream *stream,u32 now)
{ if(stream->active && !stream->sealed && now-stream->started>LIBRARY_APPLY_TTL_MS) library_apply_discard(stream); }
static int library_apply_header_matches(const struct library_apply_stream *stream,const u8 *frame,u32 length)
{
    return frame && length>=128 && length<=GRKIT_CONTROL_PAYLOAD_BYTES &&
        library_store_equal(frame,stream->header,76) && library_store_word(frame+76)==stream->received &&
        library_store_equal(frame+80,stream->header+80,48);
}
/* No callback performs I/O here. The owner validates current card/runtime state
   independently, then supplies its current manifest digest and generation. */
static int library_apply_admit(struct library_apply_stream *stream,const u8 *frame,u32 length,
                               const u32 epoch[4],const u8 card_hash[32],u32 generation,
                               const u8 before_hash[32],u32 now)
{
    if(!stream || !frame || !epoch || !card_hash || !before_hash || length<=128 || length>GRKIT_CONTROL_PAYLOAD_BYTES) return 0;
    if(library_apply_overlap(stream,sizeof(*stream),frame,length) ||
       library_apply_overlap(stream,sizeof(*stream),epoch,16) ||
       library_apply_overlap(stream,sizeof(*stream),card_hash,32) ||
       library_apply_overlap(stream,sizeof(*stream),before_hash,32)) return 0;
    library_apply_expire(stream,now);
    if(stream->active || !library_store_equal(frame,(const u8 *)"GRS1",4) || library_store_word(frame+76)) return 0;
    u32 token=0,digest=0,epoch_bits=0,card_bits=0;
    for(u32 i=0;i<4;++i) epoch_bits|=epoch[i];
    for(u32 i=0;i<32;++i) card_bits|=card_hash[i];
    for(u32 i=0;i<16;++i) token|=frame[4+i];
    for(u32 i=0;i<32;++i) digest|=frame[80+i];
    for(u32 i=112;i<128;++i) if(frame[i]) return 0;
    u32 total=library_store_word(frame+72);
    if(!token || !digest || !epoch_bits || !card_bits || total<76 || total>LIBRARY_APPLY_MAX || length-128>total ||
       library_store_word(frame+68)!=generation || !library_store_equal(frame+36,card_hash,32)) return 0;
    for(u32 i=0;i<4;++i) if(library_store_word(frame+20+4*i)!=epoch[i]) return 0;
    for(u32 i=0;i<128;++i) stream->header[i]=frame[i];
    for(u32 i=0;i<32;++i) stream->before_hash[i]=before_hash[i];
    stream->active=1; stream->sealed=stream->ready=stream->received=0;
    stream->manifest_length=stream->content_length=stream->select_off=0; stream->started=now;
    stream->manifest_buffer=stream->content_buffer=0;
    stream->manifest_capacity=stream->content_capacity=0;
    library_sha256_init(&stream->hash);
    return 1;
}
static int library_apply_byte(struct library_apply_stream *stream,const struct library_apply_buffers *buffers,u32 position,u8 value)
{
    if(position<32) return value==stream->before_hash[position];
    if(position<36) {
        stream->manifest_length|=(u32)value<<(8*(position-32));
        return position!=35 || (stream->manifest_length>=32 && stream->manifest_length<=LIBRARY_STORE_MANIFEST_MAX &&
            stream->manifest_length<=buffers->manifest_capacity && stream->manifest_length+44<=library_store_word(stream->header+72));
    }
    position-=36;
    if(position<stream->manifest_length) { buffers->manifest[position]=value; return 1; }
    position-=stream->manifest_length;
    if(position<4) {
        stream->content_length|=(u32)value<<(8*position);
        return position!=3 || ((stream->content_length==0 || stream->content_length==GRKIT_GRF_SIZE || stream->content_length==GRKIT_GRF_MAX_SIZE) &&
            stream->content_length<=buffers->content_capacity && stream->manifest_length+stream->content_length+44==library_store_word(stream->header+72));
    }
    position-=4;
    if(position<stream->content_length) { buffers->content[position]=value; return 1; }
    position-=stream->content_length;
    if(position>=4) return 0;
    stream->select_off|=(u32)value<<(8*position);
    return position!=3 || stream->select_off<=1;
}
static int library_apply_complete(struct library_apply_stream *stream,const struct library_apply_buffers *buffers,int (*validate_grf)(const u8 *,u32))
{
    u8 digest[32]; struct library_manifest manifest;
    if(!library_sha256_final(&stream->hash,digest) || !library_store_equal(digest,stream->header+80,32) ||
       !library_manifest_open(buffers->manifest,stream->manifest_length,&manifest)) return 0;
    if(stream->content_length) {
        u32 found=0;
        for(u32 i=0;i<manifest.entries;++i) {
            struct library_manifest_entry entry; library_manifest_get(&manifest,i,&entry);
            if(!(entry.flags&2U) && library_store_equal(entry.id,buffers->content+GRKIT_GRF_ID,32)) {
                if(!library_content_matches(&entry,buffers->content,stream->content_length,validate_grf)) return 0;
                ++found;
            }
        }
        if(found!=1) return 0;
    }
    stream->ready=1; return 1;
}
static int library_apply_write(struct library_apply_stream *stream,const struct library_apply_buffers *buffers,
                               const u8 *frame,u32 length,u32 now,int (*validate_grf)(const u8 *,u32),u8 acknowledgement[128])
{
    if(!stream || !buffers || !buffers->manifest || !buffers->content || !validate_grf || !acknowledgement) return 0;
    library_apply_expire(stream,now);
    if(!stream->active || stream->sealed || stream->ready || !library_apply_header_matches(stream,frame,length) || length==128 ||
       stream->received>library_store_word(stream->header+72) ||
       length-128>library_store_word(stream->header+72)-stream->received) return 0;
    /* ACK may reuse the consumed input frame, but never staged storage or
       parser state. Reject overlap before touching either destination. */
    if(library_apply_overlap(stream,sizeof(*stream),frame,length) ||
       library_apply_overlap(stream,sizeof(*stream),buffers,sizeof(*buffers)) ||
       library_apply_overlap(acknowledgement,128,buffers,sizeof(*buffers)) ||
       library_apply_overlap(stream,sizeof(*stream),acknowledgement,128) ||
       library_apply_overlap(stream,sizeof(*stream),buffers->manifest,buffers->manifest_capacity) ||
       library_apply_overlap(stream,sizeof(*stream),buffers->content,buffers->content_capacity) ||
       library_apply_overlap(buffers,sizeof(*buffers),buffers->manifest,buffers->manifest_capacity) ||
       library_apply_overlap(buffers,sizeof(*buffers),buffers->content,buffers->content_capacity) ||
       library_apply_overlap(buffers->manifest,buffers->manifest_capacity,buffers->content,buffers->content_capacity) ||
       library_apply_overlap(frame,length,buffers->manifest,buffers->manifest_capacity) ||
       library_apply_overlap(frame,length,buffers->content,buffers->content_capacity) ||
       library_apply_overlap(acknowledgement,128,buffers->manifest,buffers->manifest_capacity) ||
       library_apply_overlap(acknowledgement,128,buffers->content,buffers->content_capacity)) return 0;
    if(stream->received) {
        if(stream->manifest_buffer!=buffers->manifest || stream->content_buffer!=buffers->content ||
           stream->manifest_capacity!=buffers->manifest_capacity || stream->content_capacity!=buffers->content_capacity) return 0;
    } else {
        stream->manifest_buffer=buffers->manifest; stream->content_buffer=buffers->content;
        stream->manifest_capacity=buffers->manifest_capacity; stream->content_capacity=buffers->content_capacity;
    }
    u32 count=length-128;
    for(u32 i=0;i<count;++i) if(!library_apply_byte(stream,buffers,stream->received+i,frame[128+i])) { library_apply_discard(stream); return 0; }
    if(!library_sha256_update(&stream->hash,frame+128,count)) { library_apply_discard(stream); return 0; }
    stream->received+=count;
    if(stream->received==library_store_word(stream->header+72) && !library_apply_complete(stream,buffers,validate_grf)) { library_apply_discard(stream); return 0; }
    for(u32 i=0;i<128;++i) acknowledgement[i]=stream->header[i];
    library_store_put(acknowledgement+76,stream->received);
    return 1;
}
/* Seal only after current identity, manifest digest, dependencies and runtime
   admission are rechecked. From here the owner must explicitly recover errors;
   a timeout or a Linux disconnect cannot discard a possibly durable commit. */
static int library_apply_seal(struct library_apply_stream *stream,const u8 *request,u32 length,u32 now)
{
    if(!stream) return 0;
    library_apply_expire(stream,now);
    if(!stream->active || stream->sealed || !stream->ready || length!=128 || !library_apply_header_matches(stream,request,length)) return 0;
    stream->sealed=1; return 1;
}
static int library_apply_abort(struct library_apply_stream *stream,const u8 *request,u32 length,u32 now)
{
    if(!stream) return 0;
    library_apply_expire(stream,now);
    if(!stream->active || stream->sealed || length!=128 || !library_apply_header_matches(stream,request,length)) return 0;
    library_apply_discard(stream); return 1;
}
#endif
