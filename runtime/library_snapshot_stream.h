#ifndef GRKIT_LIBRARY_SNAPSHOT_STREAM_H
#define GRKIT_LIBRARY_SNAPSHOT_STREAM_H
#include "library_snapshot_json.h"
#include "library_content.h"
#define LIBRARY_STREAM_HEADER 128U
#define LIBRARY_STREAM_TTL_MS 15000U
struct library_snapshot_io {
    void *context;
    /* Return a complete, verified immutable GRF. Its buffer must not overlap
       the outgoing frame. It remains valid until the next load_content call. */
    int (*load_content)(void *,u32,const u8 **,u32 *);
    int (*valid_pin)(void *);
};
struct library_snapshot_stream {
    u8 header[LIBRARY_STREAM_HEADER];
    u32 active,started,next,metadata_length,content_count;
    u32 indices[64],sizes[64];
};
/* Compare lengths without overflowing a pointer-end addition. */
static int library_snapshot_overlap(const void *left,u32 left_size,const void *right,u32 right_size)
{
    __UINTPTR_TYPE__ a=(__UINTPTR_TYPE__)left,b=(__UINTPTR_TYPE__)right;
    return left_size && right_size && (a<=b ? b-a<left_size : a-b<right_size);
}
static int library_snapshot_output(const struct library_snapshot_stream *stream,
                                   const struct library_snapshot_metadata *meta,u8 *frame,u32 capacity)
{
    if(!meta || !meta->manifest || !meta->manifest->bytes || !meta->dependencies || !meta->card_id ||
       meta->manifest->entries>256 || meta->manifest->listed>64 || meta->loaded_count>40 ||
       meta->blocked_count>256 || meta->reserved_count>256 || meta->official_count>256 ||
       (meta->official_count && !meta->official)) return 0;
    u32 card_length=0; while(card_length<=128 && meta->card_id[card_length]) ++card_length;
    if(card_length>128 || library_snapshot_overlap(frame,capacity,stream,sizeof(*stream)) ||
       library_snapshot_overlap(frame,capacity,meta,sizeof(*meta)) ||
       library_snapshot_overlap(frame,capacity,meta->manifest,sizeof(*meta->manifest)) ||
       library_snapshot_overlap(frame,capacity,meta->manifest->bytes,32+96*meta->manifest->entries+2*meta->manifest->listed) ||
       library_snapshot_overlap(frame,capacity,meta->card_id,card_length+1) ||
       library_snapshot_overlap(frame,capacity,meta->dependencies,sizeof(*meta->dependencies)) ||
       library_snapshot_overlap(frame,capacity,meta->loaded,4*meta->loaded_count) ||
       library_snapshot_overlap(frame,capacity,meta->blocked,4*meta->blocked_count) ||
       library_snapshot_overlap(frame,capacity,meta->reserved_codes,4*meta->reserved_count) ||
       library_snapshot_overlap(frame,capacity,meta->official,sizeof(*meta->official)*meta->official_count)) return 0;
    for(u32 i=0;i<meta->official_count;++i)
        if(library_snapshot_overlap(frame,capacity,meta->official[i].id,32) ||
           library_snapshot_overlap(frame,capacity,meta->official[i].hash,32)) return 0;
    return 1;
}
static int library_snapshot_content(const struct library_snapshot_metadata *meta,u32 index,const u8 *bytes,u32 size)
{
    struct library_manifest_entry entry; u8 hash[32];
    return bytes && (size==GRKIT_GRF_SIZE || size==GRKIT_GRF_MAX_SIZE) &&
        library_manifest_get(meta->manifest,index,&entry) && !(entry.flags&2U) &&
        library_sha256(bytes,size,hash) && library_store_equal(hash,entry.hash,32);
}
struct library_snapshot_range { u32 offset,length,position,written; u8 *bytes; };
static int library_snapshot_range_write(void *context,const u8 *bytes,u32 length)
{
    struct library_snapshot_range *range=context;
    if(length>LIBRARY_SHA256_MAX_BYTES-range->position) return 0;
    u32 end=range->position+length,limit=range->offset+range->length;
    u32 first=range->position>range->offset?range->position:range->offset;
    u32 last=end<limit?end:limit;
    if(first<last) {
        for(u32 i=first;i<last;++i) range->bytes[range->written++]=bytes[i-range->position];
    }
    range->position=end; return 1;
}
static int library_snapshot_count(void *context,const u8 *bytes,u32 length)
{ (void)context; (void)bytes; (void)length; return 1; }
static int library_snapshot_hash(void *context,const u8 *bytes,u32 length)
{ return library_sha256_update(context,bytes,length); }
static int library_snapshot_hash_order(const u8 *left,const u8 *right)
{
    for(u32 i=0;i<32;++i) if(left[i]!=right[i]) return left[i]<right[i]?-1:1;
    return 0;
}
static int library_snapshot_live(struct library_snapshot_stream *stream,u32 now,const struct library_snapshot_io *io)
{
    if(!stream->active) return 0;
    if(now-stream->started>LIBRARY_STREAM_TTL_MS || !io->valid_pin(io->context)) { stream->active=0; return 0; }
    return 1;
}
/* Call on every resident poll, including while no Linux request is pending. */
static void library_snapshot_expire(struct library_snapshot_stream *stream,u32 now)
{ if(stream->active && now-stream->started>LIBRARY_STREAM_TTL_MS) stream->active=0; }
static int library_snapshot_request(const struct library_snapshot_stream *stream,const u8 *request,u32 length)
{
    return request && length==LIBRARY_STREAM_HEADER &&
        library_store_equal(request,stream->header,76) &&
        library_store_word(request+76)==stream->next &&
        library_store_equal(request+80,stream->header+80,48);
}
static int library_snapshot_emit(struct library_snapshot_stream *stream,const struct library_snapshot_metadata *meta,
                                 const struct library_snapshot_io *io,u32 now,u8 *frame,u32 capacity,u32 *length)
{
    if(length) *length=0;
    if(!frame || !length || capacity<=LIBRARY_STREAM_HEADER || !library_snapshot_live(stream,now,io)) return 0;
    if(capacity>GRKIT_CONTROL_PAYLOAD_BYTES) capacity=GRKIT_CONTROL_PAYLOAD_BYTES;
    u32 total=library_store_word(stream->header+72);
    if(stream->next>total || !library_snapshot_output(stream,meta,frame,capacity)) { stream->active=0; return 0; }
    u32 count=total-stream->next;
    u32 available=capacity-LIBRARY_STREAM_HEADER;
    if(available>GRKIT_CONTROL_PAYLOAD_BYTES-LIBRARY_STREAM_HEADER) available=GRKIT_CONTROL_PAYLOAD_BYTES-LIBRARY_STREAM_HEADER;
    if(count>available) count=available;
    if(!count) return 0;
    struct library_snapshot_range range={stream->next,count,0,0,frame+LIBRARY_STREAM_HEADER};
    u8 word[4]; library_store_put(word,stream->metadata_length);
    if(!library_snapshot_range_write(&range,word,4)) return 0;
    struct library_json_sink sink={&range,library_snapshot_range_write,0,0};
    if(!library_snapshot_json(meta,&sink) || sink.count!=stream->metadata_length) { stream->active=0; return 0; }
    for(u32 i=0;i<stream->content_count;++i) {
        library_store_put(word,stream->sizes[i]);
        if(!library_snapshot_range_write(&range,word,4)) { stream->active=0; return 0; }
        if(range.position<range.offset+range.length && range.position+stream->sizes[i]>range.offset) {
            const u8 *bytes; u32 size;
            if(!io->load_content(io->context,stream->indices[i],&bytes,&size) || size!=stream->sizes[i] ||
               library_snapshot_overlap(bytes,size,frame,capacity) ||
               !library_snapshot_content(meta,stream->indices[i],bytes,size) ||
               !library_snapshot_range_write(&range,bytes,size)) { stream->active=0; return 0; }
        } else range.position+=stream->sizes[i];
    }
    if(range.position!=total || range.written!=count || !library_snapshot_live(stream,now,io)) { stream->active=0; return 0; }
    for(u32 i=0;i<LIBRARY_STREAM_HEADER;++i) frame[i]=stream->header[i];
    library_store_put(frame+76,stream->next);
    stream->next+=count; *length=LIBRARY_STREAM_HEADER+count;
    return 1;
}
static int library_snapshot_begin(struct library_snapshot_stream *stream,const struct library_snapshot_metadata *meta,
                                  const u8 token[16],const struct library_snapshot_io *io,u32 now,
                                  u8 *frame,u32 capacity,u32 *length)
{
    if(length) *length=0;
    if(!stream || !meta || !token || !io || !io->load_content || !io->valid_pin ||
       !frame || !length || capacity<=LIBRARY_STREAM_HEADER) return 0;
    if(capacity>GRKIT_CONTROL_PAYLOAD_BYTES) capacity=GRKIT_CONTROL_PAYLOAD_BYTES;
    if(!library_snapshot_output(stream,meta,frame,capacity)) return 0;
    library_snapshot_expire(stream,now);
    if(stream->active || !io->valid_pin(io->context)) return 0;
    u32 any=0; for(u32 i=0;i<16;++i) any|=token[i]; if(!any) return 0;
    struct library_json_sink sink={0,library_snapshot_count,0,0};
    if(!library_snapshot_json(meta,&sink)) return 0;
    stream->metadata_length=sink.count; stream->content_count=0;
    for(u32 i=0;i<meta->manifest->entries;++i) {
        const u8 *record=library_manifest_record(meta->manifest,i);
        if(library_store_word(record+84)&2U) continue;
        u32 position=0;
        while(position<stream->content_count && library_snapshot_hash_order(library_manifest_record(meta->manifest,stream->indices[position])+52,record+52)<0) ++position;
        if(position<stream->content_count && !library_snapshot_hash_order(library_manifest_record(meta->manifest,stream->indices[position])+52,record+52)) return 0;
        if(stream->content_count>=64) return 0;
        for(u32 j=stream->content_count;j>position;--j) stream->indices[j]=stream->indices[j-1];
        stream->indices[position]=i; ++stream->content_count;
    }
    struct library_sha256_state hash; library_sha256_init(&hash);
    u8 word[4]; library_store_put(word,stream->metadata_length);
    if(!library_sha256_update(&hash,word,4)) return 0;
    sink.context=&hash; sink.write=library_snapshot_hash;
    if(!library_snapshot_json(meta,&sink) || sink.count!=stream->metadata_length) return 0;
    u32 total=4+stream->metadata_length;
    for(u32 i=0;i<stream->content_count;++i) {
        const u8 *bytes; u32 size;
        if(!io->load_content(io->context,stream->indices[i],&bytes,&size) ||
           library_snapshot_overlap(bytes,size,frame,capacity) ||
           !library_snapshot_content(meta,stream->indices[i],bytes,size)) return 0;
        stream->sizes[i]=size; library_store_put(word,size);
        if(!library_sha256_update(&hash,word,4) || !library_sha256_update(&hash,bytes,size)) return 0;
        total+=4+size;
    }
    for(u32 i=0;i<LIBRARY_STREAM_HEADER;++i) stream->header[i]=0;
    for(u32 i=0;i<4;++i) stream->header[i]=(u8)"GRS1"[i];
    for(u32 i=0;i<16;++i) stream->header[4+i]=token[i];
    for(u32 i=0;i<4;++i) library_store_put(stream->header+20+4*i,meta->epoch[i]);
    u32 card_length=0; while(meta->card_id[card_length]) ++card_length;
    if(!library_sha256((const u8 *)meta->card_id,card_length,stream->header+36) || !library_sha256_final(&hash,stream->header+80)) return 0;
    library_store_put(stream->header+68,meta->generation); library_store_put(stream->header+72,total);
    stream->started=now; stream->next=0; stream->active=1;
    if(library_snapshot_emit(stream,meta,io,now,frame,capacity,length)) return 1;
    stream->active=0; return 0;
}
static int library_snapshot_read(struct library_snapshot_stream *stream,const struct library_snapshot_metadata *meta,
                                 const struct library_snapshot_io *io,u32 now,const u8 *request,u32 request_length,
                                 u8 *frame,u32 capacity,u32 *length)
{
    if(length) *length=0;
    if(!stream || !meta || !io || !io->valid_pin || !io->load_content ||
       !library_snapshot_live(stream,now,io) || !library_snapshot_request(stream,request,request_length)) return 0;
    return library_snapshot_emit(stream,meta,io,now,frame,capacity,length);
}
static int library_snapshot_end(struct library_snapshot_stream *stream,const struct library_snapshot_io *io,u32 now,const u8 *request,u32 length)
{
    if(!stream || !io || !io->valid_pin || !library_snapshot_live(stream,now,io) ||
       stream->next!=library_store_word(stream->header+72) || !library_snapshot_request(stream,request,length)) return 0;
    stream->active=0; return 1;
}
#endif
