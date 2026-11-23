; ModuleID = 'ycbcr_convert.metal'
source_filename = "ycbcr_convert.metal"
target datalayout = "e-p:64:64:64-i1:8:8-i8:8:8-i16:16:16-i32:32:32-i64:64:64-f32:32:32-f64:64:64-v16:16:16-v24:32:32-v32:32:32-v48:64:64-v64:64:64-v96:128:128-v128:128:128-v192:256:256-v256:256:256-v512:512:512-v1024:1024:1024-n8:16:32"
target triple = "air64_v28-apple-macosx26.0.0"

%struct.YCbCrParams = type { i32, i32, i32, i32, i32, i32, i32, i32, i32, i32, i32, i32, i32, i32, i32, i32 }
%struct._texture_2d_t = type opaque

; Function Attrs: mustprogress nounwind willreturn
define void @ycbcr_convert(i32 addrspace(1)* noundef readonly "air-buffer-no-alias" %0, %struct.YCbCrParams addrspace(2)* nocapture noundef readonly align 4 dereferenceable(64) "air-buffer-no-alias" %1, %struct._texture_2d_t addrspace(1)* %2, <2 x i32> noundef %3) local_unnamed_addr #0 {
  %5 = extractelement <2 x i32> %3, i64 0
  %6 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 0
  %7 = load i32, i32 addrspace(2)* %6, align 4, !tbaa !24, !alias.scope !29, !noalias !32
  %8 = icmp ult i32 %5, %7
  br i1 %8, label %9, label %1476

9:                                                ; preds = %4
  %10 = extractelement <2 x i32> %3, i64 1
  %11 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 1
  %12 = load i32, i32 addrspace(2)* %11, align 4, !tbaa !35, !alias.scope !29, !noalias !32
  %13 = icmp ult i32 %10, %12
  br i1 %13, label %14, label %1476

14:                                               ; preds = %9
  %15 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 8
  %16 = load i32, i32 addrspace(2)* %15, align 4, !tbaa !36, !alias.scope !29, !noalias !32
  %17 = and i32 %16, 3
  %18 = lshr i32 %16, 2
  %19 = and i32 %18, 3
  %20 = lshr i32 %16, 5
  %21 = and i32 %20, 7
  %22 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 2
  %23 = load i32, i32 addrspace(2)* %22, align 4, !tbaa !37, !alias.scope !29, !noalias !32
  %24 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 3
  %25 = load i32, i32 addrspace(2)* %24, align 4, !tbaa !38, !alias.scope !29, !noalias !32
  %26 = mul i32 %25, %10
  %27 = add i32 %26, %23
  switch i32 %17, label %70 [
    i32 0, label %28
    i32 1, label %38
  ]

28:                                               ; preds = %14
  %29 = add i32 %27, %5
  %30 = lshr i32 %29, 2
  %31 = zext i32 %30 to i64
  %32 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %31
  %33 = load i32, i32 addrspace(1)* %32, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %34 = shl i32 %29, 3
  %35 = and i32 %34, 24
  %36 = lshr i32 %33, %35
  %37 = and i32 %36, 255
  br label %97

38:                                               ; preds = %14
  %39 = shl i32 %5, 1
  %40 = add i32 %27, %39
  %41 = and i32 %40, 1
  %42 = icmp eq i32 %41, 0
  %43 = lshr i32 %40, 2
  %44 = zext i32 %43 to i64
  %45 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %44
  %46 = load i32, i32 addrspace(1)* %45, align 4, !tbaa !39
  %47 = shl i32 %40, 3
  br i1 %42, label %48, label %52

48:                                               ; preds = %38
  %49 = and i32 %47, 16
  %50 = lshr i32 %46, %49
  %51 = and i32 %50, 65535
  br label %67

52:                                               ; preds = %38
  %53 = and i32 %47, 24
  %54 = lshr i32 %46, %53
  %55 = and i32 %54, 255
  %56 = add i32 %40, 1
  %57 = lshr i32 %56, 2
  %58 = zext i32 %57 to i64
  %59 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %58
  %60 = load i32, i32 addrspace(1)* %59, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %61 = shl i32 %56, 3
  %62 = and i32 %61, 24
  %63 = lshr i32 %60, %62
  %64 = shl i32 %63, 8
  %65 = and i32 %64, 65280
  %66 = or i32 %65, %55
  br label %67

67:                                               ; preds = %48, %52
  %68 = phi i32 [ %51, %48 ], [ %66, %52 ]
  %69 = lshr i32 %68, 6
  br label %97

70:                                               ; preds = %14
  %71 = udiv i32 %5, 3
  %72 = shl i32 %71, 2
  %73 = add i32 %27, %72
  %74 = and i32 %73, 3
  %75 = icmp eq i32 %74, 0
  %76 = lshr i32 %73, 2
  %77 = zext i32 %76 to i64
  %78 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %77
  %79 = load i32, i32 addrspace(1)* %78, align 4, !tbaa !39
  br i1 %75, label %91, label %80

80:                                               ; preds = %70
  %81 = add nuw nsw i32 %76, 1
  %82 = zext i32 %81 to i64
  %83 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %82
  %84 = load i32, i32 addrspace(1)* %83, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %85 = shl nuw nsw i32 %74, 3
  %86 = lshr i32 %79, %85
  %87 = sub nsw i32 0, %85
  %88 = and i32 %87, 24
  %89 = shl i32 %84, %88
  %90 = or i32 %89, %86
  br label %91

91:                                               ; preds = %70, %80
  %92 = phi i32 [ %90, %80 ], [ %79, %70 ]
  %93 = urem i32 %5, 3
  %94 = mul nuw nsw i32 %93, 10
  %95 = lshr i32 %92, %94
  %96 = and i32 %95, 1023
  br label %97

97:                                               ; preds = %28, %67, %91
  %98 = phi i32 [ %37, %28 ], [ %69, %67 ], [ %96, %91 ]
  switch i32 %19, label %1050 [
    i32 0, label %99
    i32 1, label %727
  ]

99:                                               ; preds = %97
  %100 = lshr i32 %5, 1
  %101 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 6
  %102 = load i32, i32 addrspace(2)* %101, align 4, !tbaa !42, !alias.scope !29, !noalias !32
  %103 = add i32 %102, -1
  %104 = tail call i32 @air.min.u.i32(i32 %100, i32 %103) #3
  %105 = and i32 %5, 1
  %106 = icmp eq i32 %105, 0
  br i1 %106, label %110, label %107

107:                                              ; preds = %99
  %108 = add i32 %104, 1
  %109 = tail call i32 @air.min.u.i32(i32 %108, i32 %103) #3
  br label %114

110:                                              ; preds = %99
  %111 = icmp eq i32 %104, 0
  %112 = add i32 %104, -1
  %113 = select i1 %111, i32 0, i32 %112
  br label %114

114:                                              ; preds = %110, %107
  %115 = phi i32 [ %109, %107 ], [ %113, %110 ]
  %116 = lshr i32 %10, 1
  %117 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 7
  %118 = load i32, i32 addrspace(2)* %117, align 4, !tbaa !43, !alias.scope !29, !noalias !32
  %119 = add i32 %118, -1
  %120 = tail call i32 @air.min.u.i32(i32 %116, i32 %119) #3
  %121 = and i32 %10, 1
  %122 = icmp eq i32 %121, 0
  br i1 %122, label %126, label %123

123:                                              ; preds = %114
  %124 = add i32 %120, 1
  %125 = tail call i32 @air.min.u.i32(i32 %124, i32 %119) #3
  br label %130

126:                                              ; preds = %114
  %127 = icmp eq i32 %120, 0
  %128 = add i32 %120, -1
  %129 = select i1 %127, i32 0, i32 %128
  br label %130

130:                                              ; preds = %126, %123
  %131 = phi i32 [ %125, %123 ], [ %129, %126 ]
  %132 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 4
  %133 = load i32, i32 addrspace(2)* %132, align 4, !tbaa !44, !alias.scope !29, !noalias !32
  %134 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 5
  %135 = load i32, i32 addrspace(2)* %134, align 4, !tbaa !45, !alias.scope !29, !noalias !32
  %136 = mul i32 %135, %120
  %137 = add i32 %136, %133
  %138 = shl i32 %104, 1
  switch i32 %17, label %181 [
    i32 0, label %139
    i32 1, label %149
  ]

139:                                              ; preds = %130
  %140 = add i32 %137, %138
  %141 = lshr i32 %140, 2
  %142 = zext i32 %141 to i64
  %143 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %142
  %144 = load i32, i32 addrspace(1)* %143, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %145 = shl i32 %140, 3
  %146 = and i32 %145, 24
  %147 = lshr i32 %144, %146
  %148 = and i32 %147, 255
  br label %208

149:                                              ; preds = %130
  %150 = shl i32 %104, 2
  %151 = add i32 %137, %150
  %152 = and i32 %151, 1
  %153 = icmp eq i32 %152, 0
  %154 = lshr i32 %151, 2
  %155 = zext i32 %154 to i64
  %156 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %155
  %157 = load i32, i32 addrspace(1)* %156, align 4, !tbaa !39
  %158 = shl i32 %151, 3
  br i1 %153, label %159, label %163

159:                                              ; preds = %149
  %160 = and i32 %158, 16
  %161 = lshr i32 %157, %160
  %162 = and i32 %161, 65535
  br label %178

163:                                              ; preds = %149
  %164 = and i32 %158, 24
  %165 = lshr i32 %157, %164
  %166 = and i32 %165, 255
  %167 = add i32 %151, 1
  %168 = lshr i32 %167, 2
  %169 = zext i32 %168 to i64
  %170 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %169
  %171 = load i32, i32 addrspace(1)* %170, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %172 = shl i32 %167, 3
  %173 = and i32 %172, 24
  %174 = lshr i32 %171, %173
  %175 = shl i32 %174, 8
  %176 = and i32 %175, 65280
  %177 = or i32 %176, %166
  br label %178

178:                                              ; preds = %159, %163
  %179 = phi i32 [ %162, %159 ], [ %177, %163 ]
  %180 = lshr i32 %179, 6
  br label %208

181:                                              ; preds = %130
  %182 = udiv i32 %138, 3
  %183 = shl i32 %182, 2
  %184 = add i32 %137, %183
  %185 = and i32 %184, 3
  %186 = icmp eq i32 %185, 0
  %187 = lshr i32 %184, 2
  %188 = zext i32 %187 to i64
  %189 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %188
  %190 = load i32, i32 addrspace(1)* %189, align 4, !tbaa !39
  br i1 %186, label %202, label %191

191:                                              ; preds = %181
  %192 = add nuw nsw i32 %187, 1
  %193 = zext i32 %192 to i64
  %194 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %193
  %195 = load i32, i32 addrspace(1)* %194, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %196 = shl nuw nsw i32 %185, 3
  %197 = lshr i32 %190, %196
  %198 = sub nsw i32 0, %196
  %199 = and i32 %198, 24
  %200 = shl i32 %195, %199
  %201 = or i32 %200, %197
  br label %202

202:                                              ; preds = %181, %191
  %203 = phi i32 [ %201, %191 ], [ %190, %181 ]
  %204 = urem i32 %138, 3
  %205 = mul nuw nsw i32 %204, 10
  %206 = lshr i32 %203, %205
  %207 = and i32 %206, 1023
  br label %208

208:                                              ; preds = %202, %178, %139
  %209 = phi i32 [ %148, %139 ], [ %180, %178 ], [ %207, %202 ]
  %210 = insertelement <2 x i32> undef, i32 %209, i64 0
  %211 = or i32 %138, 1
  switch i32 %17, label %254 [
    i32 0, label %212
    i32 1, label %222
  ]

212:                                              ; preds = %208
  %213 = add i32 %137, %211
  %214 = lshr i32 %213, 2
  %215 = zext i32 %214 to i64
  %216 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %215
  %217 = load i32, i32 addrspace(1)* %216, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %218 = shl i32 %213, 3
  %219 = and i32 %218, 24
  %220 = lshr i32 %217, %219
  %221 = and i32 %220, 255
  br label %281

222:                                              ; preds = %208
  %223 = shl i32 %211, 1
  %224 = add i32 %137, %223
  %225 = and i32 %224, 1
  %226 = icmp eq i32 %225, 0
  %227 = lshr i32 %224, 2
  %228 = zext i32 %227 to i64
  %229 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %228
  %230 = load i32, i32 addrspace(1)* %229, align 4, !tbaa !39
  %231 = shl i32 %224, 3
  br i1 %226, label %232, label %236

232:                                              ; preds = %222
  %233 = and i32 %231, 16
  %234 = lshr i32 %230, %233
  %235 = and i32 %234, 65535
  br label %251

236:                                              ; preds = %222
  %237 = and i32 %231, 24
  %238 = lshr i32 %230, %237
  %239 = and i32 %238, 255
  %240 = add i32 %224, 1
  %241 = lshr i32 %240, 2
  %242 = zext i32 %241 to i64
  %243 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %242
  %244 = load i32, i32 addrspace(1)* %243, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %245 = shl i32 %240, 3
  %246 = and i32 %245, 24
  %247 = lshr i32 %244, %246
  %248 = shl i32 %247, 8
  %249 = and i32 %248, 65280
  %250 = or i32 %249, %239
  br label %251

251:                                              ; preds = %232, %236
  %252 = phi i32 [ %235, %232 ], [ %250, %236 ]
  %253 = lshr i32 %252, 6
  br label %281

254:                                              ; preds = %208
  %255 = udiv i32 %211, 3
  %256 = shl i32 %255, 2
  %257 = add i32 %137, %256
  %258 = and i32 %257, 3
  %259 = icmp eq i32 %258, 0
  %260 = lshr i32 %257, 2
  %261 = zext i32 %260 to i64
  %262 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %261
  %263 = load i32, i32 addrspace(1)* %262, align 4, !tbaa !39
  br i1 %259, label %275, label %264

264:                                              ; preds = %254
  %265 = add nuw nsw i32 %260, 1
  %266 = zext i32 %265 to i64
  %267 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %266
  %268 = load i32, i32 addrspace(1)* %267, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %269 = shl nuw nsw i32 %258, 3
  %270 = lshr i32 %263, %269
  %271 = sub nsw i32 0, %269
  %272 = and i32 %271, 24
  %273 = shl i32 %268, %272
  %274 = or i32 %273, %270
  br label %275

275:                                              ; preds = %254, %264
  %276 = phi i32 [ %274, %264 ], [ %263, %254 ]
  %277 = urem i32 %211, 3
  %278 = mul nuw nsw i32 %277, 10
  %279 = lshr i32 %276, %278
  %280 = and i32 %279, 1023
  br label %281

281:                                              ; preds = %212, %251, %275
  %282 = phi i32 [ %221, %212 ], [ %253, %251 ], [ %280, %275 ]
  %283 = insertelement <2 x i32> %210, i32 %282, i64 1
  %284 = shl i32 %115, 1
  switch i32 %17, label %327 [
    i32 0, label %285
    i32 1, label %295
  ]

285:                                              ; preds = %281
  %286 = add i32 %137, %284
  %287 = lshr i32 %286, 2
  %288 = zext i32 %287 to i64
  %289 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %288
  %290 = load i32, i32 addrspace(1)* %289, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %291 = shl i32 %286, 3
  %292 = and i32 %291, 24
  %293 = lshr i32 %290, %292
  %294 = and i32 %293, 255
  br label %354

295:                                              ; preds = %281
  %296 = shl i32 %115, 2
  %297 = add i32 %137, %296
  %298 = and i32 %297, 1
  %299 = icmp eq i32 %298, 0
  %300 = lshr i32 %297, 2
  %301 = zext i32 %300 to i64
  %302 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %301
  %303 = load i32, i32 addrspace(1)* %302, align 4, !tbaa !39
  %304 = shl i32 %297, 3
  br i1 %299, label %305, label %309

305:                                              ; preds = %295
  %306 = and i32 %304, 16
  %307 = lshr i32 %303, %306
  %308 = and i32 %307, 65535
  br label %324

309:                                              ; preds = %295
  %310 = and i32 %304, 24
  %311 = lshr i32 %303, %310
  %312 = and i32 %311, 255
  %313 = add i32 %297, 1
  %314 = lshr i32 %313, 2
  %315 = zext i32 %314 to i64
  %316 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %315
  %317 = load i32, i32 addrspace(1)* %316, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %318 = shl i32 %313, 3
  %319 = and i32 %318, 24
  %320 = lshr i32 %317, %319
  %321 = shl i32 %320, 8
  %322 = and i32 %321, 65280
  %323 = or i32 %322, %312
  br label %324

324:                                              ; preds = %305, %309
  %325 = phi i32 [ %308, %305 ], [ %323, %309 ]
  %326 = lshr i32 %325, 6
  br label %354

327:                                              ; preds = %281
  %328 = udiv i32 %284, 3
  %329 = shl i32 %328, 2
  %330 = add i32 %137, %329
  %331 = and i32 %330, 3
  %332 = icmp eq i32 %331, 0
  %333 = lshr i32 %330, 2
  %334 = zext i32 %333 to i64
  %335 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %334
  %336 = load i32, i32 addrspace(1)* %335, align 4, !tbaa !39
  br i1 %332, label %348, label %337

337:                                              ; preds = %327
  %338 = add nuw nsw i32 %333, 1
  %339 = zext i32 %338 to i64
  %340 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %339
  %341 = load i32, i32 addrspace(1)* %340, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %342 = shl nuw nsw i32 %331, 3
  %343 = lshr i32 %336, %342
  %344 = sub nsw i32 0, %342
  %345 = and i32 %344, 24
  %346 = shl i32 %341, %345
  %347 = or i32 %346, %343
  br label %348

348:                                              ; preds = %327, %337
  %349 = phi i32 [ %347, %337 ], [ %336, %327 ]
  %350 = urem i32 %284, 3
  %351 = mul nuw nsw i32 %350, 10
  %352 = lshr i32 %349, %351
  %353 = and i32 %352, 1023
  br label %354

354:                                              ; preds = %348, %324, %285
  %355 = phi i32 [ %294, %285 ], [ %326, %324 ], [ %353, %348 ]
  %356 = insertelement <2 x i32> undef, i32 %355, i64 0
  %357 = or i32 %284, 1
  switch i32 %17, label %400 [
    i32 0, label %358
    i32 1, label %368
  ]

358:                                              ; preds = %354
  %359 = add i32 %137, %357
  %360 = lshr i32 %359, 2
  %361 = zext i32 %360 to i64
  %362 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %361
  %363 = load i32, i32 addrspace(1)* %362, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %364 = shl i32 %359, 3
  %365 = and i32 %364, 24
  %366 = lshr i32 %363, %365
  %367 = and i32 %366, 255
  br label %427

368:                                              ; preds = %354
  %369 = shl i32 %357, 1
  %370 = add i32 %137, %369
  %371 = and i32 %370, 1
  %372 = icmp eq i32 %371, 0
  %373 = lshr i32 %370, 2
  %374 = zext i32 %373 to i64
  %375 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %374
  %376 = load i32, i32 addrspace(1)* %375, align 4, !tbaa !39
  %377 = shl i32 %370, 3
  br i1 %372, label %378, label %382

378:                                              ; preds = %368
  %379 = and i32 %377, 16
  %380 = lshr i32 %376, %379
  %381 = and i32 %380, 65535
  br label %397

382:                                              ; preds = %368
  %383 = and i32 %377, 24
  %384 = lshr i32 %376, %383
  %385 = and i32 %384, 255
  %386 = add i32 %370, 1
  %387 = lshr i32 %386, 2
  %388 = zext i32 %387 to i64
  %389 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %388
  %390 = load i32, i32 addrspace(1)* %389, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %391 = shl i32 %386, 3
  %392 = and i32 %391, 24
  %393 = lshr i32 %390, %392
  %394 = shl i32 %393, 8
  %395 = and i32 %394, 65280
  %396 = or i32 %395, %385
  br label %397

397:                                              ; preds = %378, %382
  %398 = phi i32 [ %381, %378 ], [ %396, %382 ]
  %399 = lshr i32 %398, 6
  br label %427

400:                                              ; preds = %354
  %401 = udiv i32 %357, 3
  %402 = shl i32 %401, 2
  %403 = add i32 %137, %402
  %404 = and i32 %403, 3
  %405 = icmp eq i32 %404, 0
  %406 = lshr i32 %403, 2
  %407 = zext i32 %406 to i64
  %408 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %407
  %409 = load i32, i32 addrspace(1)* %408, align 4, !tbaa !39
  br i1 %405, label %421, label %410

410:                                              ; preds = %400
  %411 = add nuw nsw i32 %406, 1
  %412 = zext i32 %411 to i64
  %413 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %412
  %414 = load i32, i32 addrspace(1)* %413, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %415 = shl nuw nsw i32 %404, 3
  %416 = lshr i32 %409, %415
  %417 = sub nsw i32 0, %415
  %418 = and i32 %417, 24
  %419 = shl i32 %414, %418
  %420 = or i32 %419, %416
  br label %421

421:                                              ; preds = %400, %410
  %422 = phi i32 [ %420, %410 ], [ %409, %400 ]
  %423 = urem i32 %357, 3
  %424 = mul nuw nsw i32 %423, 10
  %425 = lshr i32 %422, %424
  %426 = and i32 %425, 1023
  br label %427

427:                                              ; preds = %358, %397, %421
  %428 = phi i32 [ %367, %358 ], [ %399, %397 ], [ %426, %421 ]
  %429 = insertelement <2 x i32> %356, i32 %428, i64 1
  %430 = mul i32 %135, %131
  %431 = add i32 %430, %133
  switch i32 %17, label %474 [
    i32 0, label %432
    i32 1, label %442
  ]

432:                                              ; preds = %427
  %433 = add i32 %431, %138
  %434 = lshr i32 %433, 2
  %435 = zext i32 %434 to i64
  %436 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %435
  %437 = load i32, i32 addrspace(1)* %436, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %438 = shl i32 %433, 3
  %439 = and i32 %438, 24
  %440 = lshr i32 %437, %439
  %441 = and i32 %440, 255
  br label %501

442:                                              ; preds = %427
  %443 = shl i32 %104, 2
  %444 = add i32 %431, %443
  %445 = and i32 %444, 1
  %446 = icmp eq i32 %445, 0
  %447 = lshr i32 %444, 2
  %448 = zext i32 %447 to i64
  %449 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %448
  %450 = load i32, i32 addrspace(1)* %449, align 4, !tbaa !39
  %451 = shl i32 %444, 3
  br i1 %446, label %452, label %456

452:                                              ; preds = %442
  %453 = and i32 %451, 16
  %454 = lshr i32 %450, %453
  %455 = and i32 %454, 65535
  br label %471

456:                                              ; preds = %442
  %457 = and i32 %451, 24
  %458 = lshr i32 %450, %457
  %459 = and i32 %458, 255
  %460 = add i32 %444, 1
  %461 = lshr i32 %460, 2
  %462 = zext i32 %461 to i64
  %463 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %462
  %464 = load i32, i32 addrspace(1)* %463, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %465 = shl i32 %460, 3
  %466 = and i32 %465, 24
  %467 = lshr i32 %464, %466
  %468 = shl i32 %467, 8
  %469 = and i32 %468, 65280
  %470 = or i32 %469, %459
  br label %471

471:                                              ; preds = %452, %456
  %472 = phi i32 [ %455, %452 ], [ %470, %456 ]
  %473 = lshr i32 %472, 6
  br label %501

474:                                              ; preds = %427
  %475 = udiv i32 %138, 3
  %476 = shl i32 %475, 2
  %477 = add i32 %431, %476
  %478 = and i32 %477, 3
  %479 = icmp eq i32 %478, 0
  %480 = lshr i32 %477, 2
  %481 = zext i32 %480 to i64
  %482 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %481
  %483 = load i32, i32 addrspace(1)* %482, align 4, !tbaa !39
  br i1 %479, label %495, label %484

484:                                              ; preds = %474
  %485 = add nuw nsw i32 %480, 1
  %486 = zext i32 %485 to i64
  %487 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %486
  %488 = load i32, i32 addrspace(1)* %487, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %489 = shl nuw nsw i32 %478, 3
  %490 = lshr i32 %483, %489
  %491 = sub nsw i32 0, %489
  %492 = and i32 %491, 24
  %493 = shl i32 %488, %492
  %494 = or i32 %493, %490
  br label %495

495:                                              ; preds = %474, %484
  %496 = phi i32 [ %494, %484 ], [ %483, %474 ]
  %497 = urem i32 %138, 3
  %498 = mul nuw nsw i32 %497, 10
  %499 = lshr i32 %496, %498
  %500 = and i32 %499, 1023
  br label %501

501:                                              ; preds = %495, %471, %432
  %502 = phi i32 [ %441, %432 ], [ %473, %471 ], [ %500, %495 ]
  %503 = insertelement <2 x i32> undef, i32 %502, i64 0
  switch i32 %17, label %546 [
    i32 0, label %504
    i32 1, label %514
  ]

504:                                              ; preds = %501
  %505 = add i32 %431, %211
  %506 = lshr i32 %505, 2
  %507 = zext i32 %506 to i64
  %508 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %507
  %509 = load i32, i32 addrspace(1)* %508, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %510 = shl i32 %505, 3
  %511 = and i32 %510, 24
  %512 = lshr i32 %509, %511
  %513 = and i32 %512, 255
  br label %573

514:                                              ; preds = %501
  %515 = shl i32 %211, 1
  %516 = add i32 %431, %515
  %517 = and i32 %516, 1
  %518 = icmp eq i32 %517, 0
  %519 = lshr i32 %516, 2
  %520 = zext i32 %519 to i64
  %521 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %520
  %522 = load i32, i32 addrspace(1)* %521, align 4, !tbaa !39
  %523 = shl i32 %516, 3
  br i1 %518, label %524, label %528

524:                                              ; preds = %514
  %525 = and i32 %523, 16
  %526 = lshr i32 %522, %525
  %527 = and i32 %526, 65535
  br label %543

528:                                              ; preds = %514
  %529 = and i32 %523, 24
  %530 = lshr i32 %522, %529
  %531 = and i32 %530, 255
  %532 = add i32 %516, 1
  %533 = lshr i32 %532, 2
  %534 = zext i32 %533 to i64
  %535 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %534
  %536 = load i32, i32 addrspace(1)* %535, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %537 = shl i32 %532, 3
  %538 = and i32 %537, 24
  %539 = lshr i32 %536, %538
  %540 = shl i32 %539, 8
  %541 = and i32 %540, 65280
  %542 = or i32 %541, %531
  br label %543

543:                                              ; preds = %524, %528
  %544 = phi i32 [ %527, %524 ], [ %542, %528 ]
  %545 = lshr i32 %544, 6
  br label %573

546:                                              ; preds = %501
  %547 = udiv i32 %211, 3
  %548 = shl i32 %547, 2
  %549 = add i32 %431, %548
  %550 = and i32 %549, 3
  %551 = icmp eq i32 %550, 0
  %552 = lshr i32 %549, 2
  %553 = zext i32 %552 to i64
  %554 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %553
  %555 = load i32, i32 addrspace(1)* %554, align 4, !tbaa !39
  br i1 %551, label %567, label %556

556:                                              ; preds = %546
  %557 = add nuw nsw i32 %552, 1
  %558 = zext i32 %557 to i64
  %559 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %558
  %560 = load i32, i32 addrspace(1)* %559, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %561 = shl nuw nsw i32 %550, 3
  %562 = lshr i32 %555, %561
  %563 = sub nsw i32 0, %561
  %564 = and i32 %563, 24
  %565 = shl i32 %560, %564
  %566 = or i32 %565, %562
  br label %567

567:                                              ; preds = %546, %556
  %568 = phi i32 [ %566, %556 ], [ %555, %546 ]
  %569 = urem i32 %211, 3
  %570 = mul nuw nsw i32 %569, 10
  %571 = lshr i32 %568, %570
  %572 = and i32 %571, 1023
  br label %573

573:                                              ; preds = %504, %543, %567
  %574 = phi i32 [ %513, %504 ], [ %545, %543 ], [ %572, %567 ]
  %575 = insertelement <2 x i32> %503, i32 %574, i64 1
  switch i32 %17, label %618 [
    i32 0, label %576
    i32 1, label %586
  ]

576:                                              ; preds = %573
  %577 = add i32 %431, %284
  %578 = lshr i32 %577, 2
  %579 = zext i32 %578 to i64
  %580 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %579
  %581 = load i32, i32 addrspace(1)* %580, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %582 = shl i32 %577, 3
  %583 = and i32 %582, 24
  %584 = lshr i32 %581, %583
  %585 = and i32 %584, 255
  br label %645

586:                                              ; preds = %573
  %587 = shl i32 %115, 2
  %588 = add i32 %431, %587
  %589 = and i32 %588, 1
  %590 = icmp eq i32 %589, 0
  %591 = lshr i32 %588, 2
  %592 = zext i32 %591 to i64
  %593 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %592
  %594 = load i32, i32 addrspace(1)* %593, align 4, !tbaa !39
  %595 = shl i32 %588, 3
  br i1 %590, label %596, label %600

596:                                              ; preds = %586
  %597 = and i32 %595, 16
  %598 = lshr i32 %594, %597
  %599 = and i32 %598, 65535
  br label %615

600:                                              ; preds = %586
  %601 = and i32 %595, 24
  %602 = lshr i32 %594, %601
  %603 = and i32 %602, 255
  %604 = add i32 %588, 1
  %605 = lshr i32 %604, 2
  %606 = zext i32 %605 to i64
  %607 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %606
  %608 = load i32, i32 addrspace(1)* %607, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %609 = shl i32 %604, 3
  %610 = and i32 %609, 24
  %611 = lshr i32 %608, %610
  %612 = shl i32 %611, 8
  %613 = and i32 %612, 65280
  %614 = or i32 %613, %603
  br label %615

615:                                              ; preds = %596, %600
  %616 = phi i32 [ %599, %596 ], [ %614, %600 ]
  %617 = lshr i32 %616, 6
  br label %645

618:                                              ; preds = %573
  %619 = udiv i32 %284, 3
  %620 = shl i32 %619, 2
  %621 = add i32 %431, %620
  %622 = and i32 %621, 3
  %623 = icmp eq i32 %622, 0
  %624 = lshr i32 %621, 2
  %625 = zext i32 %624 to i64
  %626 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %625
  %627 = load i32, i32 addrspace(1)* %626, align 4, !tbaa !39
  br i1 %623, label %639, label %628

628:                                              ; preds = %618
  %629 = add nuw nsw i32 %624, 1
  %630 = zext i32 %629 to i64
  %631 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %630
  %632 = load i32, i32 addrspace(1)* %631, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %633 = shl nuw nsw i32 %622, 3
  %634 = lshr i32 %627, %633
  %635 = sub nsw i32 0, %633
  %636 = and i32 %635, 24
  %637 = shl i32 %632, %636
  %638 = or i32 %637, %634
  br label %639

639:                                              ; preds = %618, %628
  %640 = phi i32 [ %638, %628 ], [ %627, %618 ]
  %641 = urem i32 %284, 3
  %642 = mul nuw nsw i32 %641, 10
  %643 = lshr i32 %640, %642
  %644 = and i32 %643, 1023
  br label %645

645:                                              ; preds = %639, %615, %576
  %646 = phi i32 [ %585, %576 ], [ %617, %615 ], [ %644, %639 ]
  %647 = insertelement <2 x i32> undef, i32 %646, i64 0
  switch i32 %17, label %690 [
    i32 0, label %648
    i32 1, label %658
  ]

648:                                              ; preds = %645
  %649 = add i32 %431, %357
  %650 = lshr i32 %649, 2
  %651 = zext i32 %650 to i64
  %652 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %651
  %653 = load i32, i32 addrspace(1)* %652, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %654 = shl i32 %649, 3
  %655 = and i32 %654, 24
  %656 = lshr i32 %653, %655
  %657 = and i32 %656, 255
  br label %717

658:                                              ; preds = %645
  %659 = shl i32 %357, 1
  %660 = add i32 %431, %659
  %661 = and i32 %660, 1
  %662 = icmp eq i32 %661, 0
  %663 = lshr i32 %660, 2
  %664 = zext i32 %663 to i64
  %665 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %664
  %666 = load i32, i32 addrspace(1)* %665, align 4, !tbaa !39
  %667 = shl i32 %660, 3
  br i1 %662, label %668, label %672

668:                                              ; preds = %658
  %669 = and i32 %667, 16
  %670 = lshr i32 %666, %669
  %671 = and i32 %670, 65535
  br label %687

672:                                              ; preds = %658
  %673 = and i32 %667, 24
  %674 = lshr i32 %666, %673
  %675 = and i32 %674, 255
  %676 = add i32 %660, 1
  %677 = lshr i32 %676, 2
  %678 = zext i32 %677 to i64
  %679 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %678
  %680 = load i32, i32 addrspace(1)* %679, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %681 = shl i32 %676, 3
  %682 = and i32 %681, 24
  %683 = lshr i32 %680, %682
  %684 = shl i32 %683, 8
  %685 = and i32 %684, 65280
  %686 = or i32 %685, %675
  br label %687

687:                                              ; preds = %668, %672
  %688 = phi i32 [ %671, %668 ], [ %686, %672 ]
  %689 = lshr i32 %688, 6
  br label %717

690:                                              ; preds = %645
  %691 = udiv i32 %357, 3
  %692 = shl i32 %691, 2
  %693 = add i32 %431, %692
  %694 = and i32 %693, 3
  %695 = icmp eq i32 %694, 0
  %696 = lshr i32 %693, 2
  %697 = zext i32 %696 to i64
  %698 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %697
  %699 = load i32, i32 addrspace(1)* %698, align 4, !tbaa !39
  br i1 %695, label %711, label %700

700:                                              ; preds = %690
  %701 = add nuw nsw i32 %696, 1
  %702 = zext i32 %701 to i64
  %703 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %702
  %704 = load i32, i32 addrspace(1)* %703, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %705 = shl nuw nsw i32 %694, 3
  %706 = lshr i32 %699, %705
  %707 = sub nsw i32 0, %705
  %708 = and i32 %707, 24
  %709 = shl i32 %704, %708
  %710 = or i32 %709, %706
  br label %711

711:                                              ; preds = %690, %700
  %712 = phi i32 [ %710, %700 ], [ %699, %690 ]
  %713 = urem i32 %357, 3
  %714 = mul nuw nsw i32 %713, 10
  %715 = lshr i32 %712, %714
  %716 = and i32 %715, 1023
  br label %717

717:                                              ; preds = %648, %687, %711
  %718 = phi i32 [ %657, %648 ], [ %689, %687 ], [ %716, %711 ]
  %719 = insertelement <2 x i32> %647, i32 %718, i64 1
  %720 = mul nuw nsw <2 x i32> %283, <i32 9, i32 9>
  %721 = add nuw nsw <2 x i32> %575, %429
  %722 = mul nuw nsw <2 x i32> %721, <i32 3, i32 3>
  %723 = add nuw nsw <2 x i32> %720, <i32 8, i32 8>
  %724 = add nuw <2 x i32> %723, %722
  %725 = add <2 x i32> %724, %719
  %726 = lshr <2 x i32> %725, <i32 4, i32 4>
  br label %1211

727:                                              ; preds = %97
  %728 = lshr i32 %5, 1
  %729 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 6
  %730 = load i32, i32 addrspace(2)* %729, align 4, !tbaa !42, !alias.scope !29, !noalias !32
  %731 = add i32 %730, -1
  %732 = tail call i32 @air.min.u.i32(i32 %728, i32 %731) #3
  %733 = and i32 %5, 1
  %734 = icmp eq i32 %733, 0
  br i1 %734, label %738, label %735

735:                                              ; preds = %727
  %736 = add i32 %732, 1
  %737 = tail call i32 @air.min.u.i32(i32 %736, i32 %731) #3
  br label %742

738:                                              ; preds = %727
  %739 = icmp eq i32 %732, 0
  %740 = add i32 %732, -1
  %741 = select i1 %739, i32 0, i32 %740
  br label %742

742:                                              ; preds = %738, %735
  %743 = phi i32 [ %737, %735 ], [ %741, %738 ]
  %744 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 7
  %745 = load i32, i32 addrspace(2)* %744, align 4, !tbaa !43, !alias.scope !29, !noalias !32
  %746 = add i32 %745, -1
  %747 = tail call i32 @air.min.u.i32(i32 %10, i32 %746) #3
  %748 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 4
  %749 = load i32, i32 addrspace(2)* %748, align 4, !tbaa !44, !alias.scope !29, !noalias !32
  %750 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 5
  %751 = load i32, i32 addrspace(2)* %750, align 4, !tbaa !45, !alias.scope !29, !noalias !32
  %752 = mul i32 %751, %747
  %753 = add i32 %752, %749
  %754 = shl i32 %732, 1
  switch i32 %17, label %797 [
    i32 0, label %755
    i32 1, label %765
  ]

755:                                              ; preds = %742
  %756 = add i32 %753, %754
  %757 = lshr i32 %756, 2
  %758 = zext i32 %757 to i64
  %759 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %758
  %760 = load i32, i32 addrspace(1)* %759, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %761 = shl i32 %756, 3
  %762 = and i32 %761, 24
  %763 = lshr i32 %760, %762
  %764 = and i32 %763, 255
  br label %824

765:                                              ; preds = %742
  %766 = shl i32 %732, 2
  %767 = add i32 %753, %766
  %768 = and i32 %767, 1
  %769 = icmp eq i32 %768, 0
  %770 = lshr i32 %767, 2
  %771 = zext i32 %770 to i64
  %772 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %771
  %773 = load i32, i32 addrspace(1)* %772, align 4, !tbaa !39
  %774 = shl i32 %767, 3
  br i1 %769, label %775, label %779

775:                                              ; preds = %765
  %776 = and i32 %774, 16
  %777 = lshr i32 %773, %776
  %778 = and i32 %777, 65535
  br label %794

779:                                              ; preds = %765
  %780 = and i32 %774, 24
  %781 = lshr i32 %773, %780
  %782 = and i32 %781, 255
  %783 = add i32 %767, 1
  %784 = lshr i32 %783, 2
  %785 = zext i32 %784 to i64
  %786 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %785
  %787 = load i32, i32 addrspace(1)* %786, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %788 = shl i32 %783, 3
  %789 = and i32 %788, 24
  %790 = lshr i32 %787, %789
  %791 = shl i32 %790, 8
  %792 = and i32 %791, 65280
  %793 = or i32 %792, %782
  br label %794

794:                                              ; preds = %775, %779
  %795 = phi i32 [ %778, %775 ], [ %793, %779 ]
  %796 = lshr i32 %795, 6
  br label %824

797:                                              ; preds = %742
  %798 = udiv i32 %754, 3
  %799 = shl i32 %798, 2
  %800 = add i32 %753, %799
  %801 = and i32 %800, 3
  %802 = icmp eq i32 %801, 0
  %803 = lshr i32 %800, 2
  %804 = zext i32 %803 to i64
  %805 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %804
  %806 = load i32, i32 addrspace(1)* %805, align 4, !tbaa !39
  br i1 %802, label %818, label %807

807:                                              ; preds = %797
  %808 = add nuw nsw i32 %803, 1
  %809 = zext i32 %808 to i64
  %810 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %809
  %811 = load i32, i32 addrspace(1)* %810, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %812 = shl nuw nsw i32 %801, 3
  %813 = lshr i32 %806, %812
  %814 = sub nsw i32 0, %812
  %815 = and i32 %814, 24
  %816 = shl i32 %811, %815
  %817 = or i32 %816, %813
  br label %818

818:                                              ; preds = %797, %807
  %819 = phi i32 [ %817, %807 ], [ %806, %797 ]
  %820 = urem i32 %754, 3
  %821 = mul nuw nsw i32 %820, 10
  %822 = lshr i32 %819, %821
  %823 = and i32 %822, 1023
  br label %824

824:                                              ; preds = %818, %794, %755
  %825 = phi i32 [ %764, %755 ], [ %796, %794 ], [ %823, %818 ]
  %826 = insertelement <2 x i32> undef, i32 %825, i64 0
  %827 = or i32 %754, 1
  switch i32 %17, label %870 [
    i32 0, label %828
    i32 1, label %838
  ]

828:                                              ; preds = %824
  %829 = add i32 %753, %827
  %830 = lshr i32 %829, 2
  %831 = zext i32 %830 to i64
  %832 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %831
  %833 = load i32, i32 addrspace(1)* %832, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %834 = shl i32 %829, 3
  %835 = and i32 %834, 24
  %836 = lshr i32 %833, %835
  %837 = and i32 %836, 255
  br label %897

838:                                              ; preds = %824
  %839 = shl i32 %827, 1
  %840 = add i32 %753, %839
  %841 = and i32 %840, 1
  %842 = icmp eq i32 %841, 0
  %843 = lshr i32 %840, 2
  %844 = zext i32 %843 to i64
  %845 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %844
  %846 = load i32, i32 addrspace(1)* %845, align 4, !tbaa !39
  %847 = shl i32 %840, 3
  br i1 %842, label %848, label %852

848:                                              ; preds = %838
  %849 = and i32 %847, 16
  %850 = lshr i32 %846, %849
  %851 = and i32 %850, 65535
  br label %867

852:                                              ; preds = %838
  %853 = and i32 %847, 24
  %854 = lshr i32 %846, %853
  %855 = and i32 %854, 255
  %856 = add i32 %840, 1
  %857 = lshr i32 %856, 2
  %858 = zext i32 %857 to i64
  %859 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %858
  %860 = load i32, i32 addrspace(1)* %859, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %861 = shl i32 %856, 3
  %862 = and i32 %861, 24
  %863 = lshr i32 %860, %862
  %864 = shl i32 %863, 8
  %865 = and i32 %864, 65280
  %866 = or i32 %865, %855
  br label %867

867:                                              ; preds = %848, %852
  %868 = phi i32 [ %851, %848 ], [ %866, %852 ]
  %869 = lshr i32 %868, 6
  br label %897

870:                                              ; preds = %824
  %871 = udiv i32 %827, 3
  %872 = shl i32 %871, 2
  %873 = add i32 %753, %872
  %874 = and i32 %873, 3
  %875 = icmp eq i32 %874, 0
  %876 = lshr i32 %873, 2
  %877 = zext i32 %876 to i64
  %878 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %877
  %879 = load i32, i32 addrspace(1)* %878, align 4, !tbaa !39
  br i1 %875, label %891, label %880

880:                                              ; preds = %870
  %881 = add nuw nsw i32 %876, 1
  %882 = zext i32 %881 to i64
  %883 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %882
  %884 = load i32, i32 addrspace(1)* %883, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %885 = shl nuw nsw i32 %874, 3
  %886 = lshr i32 %879, %885
  %887 = sub nsw i32 0, %885
  %888 = and i32 %887, 24
  %889 = shl i32 %884, %888
  %890 = or i32 %889, %886
  br label %891

891:                                              ; preds = %870, %880
  %892 = phi i32 [ %890, %880 ], [ %879, %870 ]
  %893 = urem i32 %827, 3
  %894 = mul nuw nsw i32 %893, 10
  %895 = lshr i32 %892, %894
  %896 = and i32 %895, 1023
  br label %897

897:                                              ; preds = %828, %867, %891
  %898 = phi i32 [ %837, %828 ], [ %869, %867 ], [ %896, %891 ]
  %899 = insertelement <2 x i32> %826, i32 %898, i64 1
  %900 = shl i32 %743, 1
  switch i32 %17, label %943 [
    i32 0, label %901
    i32 1, label %911
  ]

901:                                              ; preds = %897
  %902 = add i32 %753, %900
  %903 = lshr i32 %902, 2
  %904 = zext i32 %903 to i64
  %905 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %904
  %906 = load i32, i32 addrspace(1)* %905, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %907 = shl i32 %902, 3
  %908 = and i32 %907, 24
  %909 = lshr i32 %906, %908
  %910 = and i32 %909, 255
  br label %970

911:                                              ; preds = %897
  %912 = shl i32 %743, 2
  %913 = add i32 %753, %912
  %914 = and i32 %913, 1
  %915 = icmp eq i32 %914, 0
  %916 = lshr i32 %913, 2
  %917 = zext i32 %916 to i64
  %918 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %917
  %919 = load i32, i32 addrspace(1)* %918, align 4, !tbaa !39
  %920 = shl i32 %913, 3
  br i1 %915, label %921, label %925

921:                                              ; preds = %911
  %922 = and i32 %920, 16
  %923 = lshr i32 %919, %922
  %924 = and i32 %923, 65535
  br label %940

925:                                              ; preds = %911
  %926 = and i32 %920, 24
  %927 = lshr i32 %919, %926
  %928 = and i32 %927, 255
  %929 = add i32 %913, 1
  %930 = lshr i32 %929, 2
  %931 = zext i32 %930 to i64
  %932 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %931
  %933 = load i32, i32 addrspace(1)* %932, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %934 = shl i32 %929, 3
  %935 = and i32 %934, 24
  %936 = lshr i32 %933, %935
  %937 = shl i32 %936, 8
  %938 = and i32 %937, 65280
  %939 = or i32 %938, %928
  br label %940

940:                                              ; preds = %921, %925
  %941 = phi i32 [ %924, %921 ], [ %939, %925 ]
  %942 = lshr i32 %941, 6
  br label %970

943:                                              ; preds = %897
  %944 = udiv i32 %900, 3
  %945 = shl i32 %944, 2
  %946 = add i32 %753, %945
  %947 = and i32 %946, 3
  %948 = icmp eq i32 %947, 0
  %949 = lshr i32 %946, 2
  %950 = zext i32 %949 to i64
  %951 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %950
  %952 = load i32, i32 addrspace(1)* %951, align 4, !tbaa !39
  br i1 %948, label %964, label %953

953:                                              ; preds = %943
  %954 = add nuw nsw i32 %949, 1
  %955 = zext i32 %954 to i64
  %956 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %955
  %957 = load i32, i32 addrspace(1)* %956, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %958 = shl nuw nsw i32 %947, 3
  %959 = lshr i32 %952, %958
  %960 = sub nsw i32 0, %958
  %961 = and i32 %960, 24
  %962 = shl i32 %957, %961
  %963 = or i32 %962, %959
  br label %964

964:                                              ; preds = %943, %953
  %965 = phi i32 [ %963, %953 ], [ %952, %943 ]
  %966 = urem i32 %900, 3
  %967 = mul nuw nsw i32 %966, 10
  %968 = lshr i32 %965, %967
  %969 = and i32 %968, 1023
  br label %970

970:                                              ; preds = %964, %940, %901
  %971 = phi i32 [ %910, %901 ], [ %942, %940 ], [ %969, %964 ]
  %972 = insertelement <2 x i32> undef, i32 %971, i64 0
  %973 = or i32 %900, 1
  switch i32 %17, label %1016 [
    i32 0, label %974
    i32 1, label %984
  ]

974:                                              ; preds = %970
  %975 = add i32 %753, %973
  %976 = lshr i32 %975, 2
  %977 = zext i32 %976 to i64
  %978 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %977
  %979 = load i32, i32 addrspace(1)* %978, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %980 = shl i32 %975, 3
  %981 = and i32 %980, 24
  %982 = lshr i32 %979, %981
  %983 = and i32 %982, 255
  br label %1043

984:                                              ; preds = %970
  %985 = shl i32 %973, 1
  %986 = add i32 %753, %985
  %987 = and i32 %986, 1
  %988 = icmp eq i32 %987, 0
  %989 = lshr i32 %986, 2
  %990 = zext i32 %989 to i64
  %991 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %990
  %992 = load i32, i32 addrspace(1)* %991, align 4, !tbaa !39
  %993 = shl i32 %986, 3
  br i1 %988, label %994, label %998

994:                                              ; preds = %984
  %995 = and i32 %993, 16
  %996 = lshr i32 %992, %995
  %997 = and i32 %996, 65535
  br label %1013

998:                                              ; preds = %984
  %999 = and i32 %993, 24
  %1000 = lshr i32 %992, %999
  %1001 = and i32 %1000, 255
  %1002 = add i32 %986, 1
  %1003 = lshr i32 %1002, 2
  %1004 = zext i32 %1003 to i64
  %1005 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %1004
  %1006 = load i32, i32 addrspace(1)* %1005, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %1007 = shl i32 %1002, 3
  %1008 = and i32 %1007, 24
  %1009 = lshr i32 %1006, %1008
  %1010 = shl i32 %1009, 8
  %1011 = and i32 %1010, 65280
  %1012 = or i32 %1011, %1001
  br label %1013

1013:                                             ; preds = %994, %998
  %1014 = phi i32 [ %997, %994 ], [ %1012, %998 ]
  %1015 = lshr i32 %1014, 6
  br label %1043

1016:                                             ; preds = %970
  %1017 = udiv i32 %973, 3
  %1018 = shl i32 %1017, 2
  %1019 = add i32 %753, %1018
  %1020 = and i32 %1019, 3
  %1021 = icmp eq i32 %1020, 0
  %1022 = lshr i32 %1019, 2
  %1023 = zext i32 %1022 to i64
  %1024 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %1023
  %1025 = load i32, i32 addrspace(1)* %1024, align 4, !tbaa !39
  br i1 %1021, label %1037, label %1026

1026:                                             ; preds = %1016
  %1027 = add nuw nsw i32 %1022, 1
  %1028 = zext i32 %1027 to i64
  %1029 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %1028
  %1030 = load i32, i32 addrspace(1)* %1029, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %1031 = shl nuw nsw i32 %1020, 3
  %1032 = lshr i32 %1025, %1031
  %1033 = sub nsw i32 0, %1031
  %1034 = and i32 %1033, 24
  %1035 = shl i32 %1030, %1034
  %1036 = or i32 %1035, %1032
  br label %1037

1037:                                             ; preds = %1016, %1026
  %1038 = phi i32 [ %1036, %1026 ], [ %1025, %1016 ]
  %1039 = urem i32 %973, 3
  %1040 = mul nuw nsw i32 %1039, 10
  %1041 = lshr i32 %1038, %1040
  %1042 = and i32 %1041, 1023
  br label %1043

1043:                                             ; preds = %974, %1013, %1037
  %1044 = phi i32 [ %983, %974 ], [ %1015, %1013 ], [ %1042, %1037 ]
  %1045 = insertelement <2 x i32> %972, i32 %1044, i64 1
  %1046 = mul nuw nsw <2 x i32> %899, <i32 3, i32 3>
  %1047 = add nuw nsw <2 x i32> %1046, <i32 2, i32 2>
  %1048 = add nuw nsw <2 x i32> %1047, %1045
  %1049 = lshr <2 x i32> %1048, <i32 2, i32 2>
  br label %1211

1050:                                             ; preds = %97
  %1051 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 6
  %1052 = load i32, i32 addrspace(2)* %1051, align 4, !tbaa !42, !alias.scope !29, !noalias !32
  %1053 = add i32 %1052, -1
  %1054 = tail call i32 @air.min.u.i32(i32 %5, i32 %1053) #3
  %1055 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 7
  %1056 = load i32, i32 addrspace(2)* %1055, align 4, !tbaa !43, !alias.scope !29, !noalias !32
  %1057 = add i32 %1056, -1
  %1058 = tail call i32 @air.min.u.i32(i32 %10, i32 %1057) #3
  %1059 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 4
  %1060 = load i32, i32 addrspace(2)* %1059, align 4, !tbaa !44, !alias.scope !29, !noalias !32
  %1061 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 5
  %1062 = load i32, i32 addrspace(2)* %1061, align 4, !tbaa !45, !alias.scope !29, !noalias !32
  %1063 = mul i32 %1062, %1058
  %1064 = add i32 %1063, %1060
  %1065 = shl i32 %1054, 1
  switch i32 %17, label %1108 [
    i32 0, label %1066
    i32 1, label %1076
  ]

1066:                                             ; preds = %1050
  %1067 = add i32 %1064, %1065
  %1068 = lshr i32 %1067, 2
  %1069 = zext i32 %1068 to i64
  %1070 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %1069
  %1071 = load i32, i32 addrspace(1)* %1070, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %1072 = shl i32 %1067, 3
  %1073 = and i32 %1072, 24
  %1074 = lshr i32 %1071, %1073
  %1075 = and i32 %1074, 255
  br label %1135

1076:                                             ; preds = %1050
  %1077 = shl i32 %1054, 2
  %1078 = add i32 %1064, %1077
  %1079 = and i32 %1078, 1
  %1080 = icmp eq i32 %1079, 0
  %1081 = lshr i32 %1078, 2
  %1082 = zext i32 %1081 to i64
  %1083 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %1082
  %1084 = load i32, i32 addrspace(1)* %1083, align 4, !tbaa !39
  %1085 = shl i32 %1078, 3
  br i1 %1080, label %1086, label %1090

1086:                                             ; preds = %1076
  %1087 = and i32 %1085, 16
  %1088 = lshr i32 %1084, %1087
  %1089 = and i32 %1088, 65535
  br label %1105

1090:                                             ; preds = %1076
  %1091 = and i32 %1085, 24
  %1092 = lshr i32 %1084, %1091
  %1093 = and i32 %1092, 255
  %1094 = add i32 %1078, 1
  %1095 = lshr i32 %1094, 2
  %1096 = zext i32 %1095 to i64
  %1097 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %1096
  %1098 = load i32, i32 addrspace(1)* %1097, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %1099 = shl i32 %1094, 3
  %1100 = and i32 %1099, 24
  %1101 = lshr i32 %1098, %1100
  %1102 = shl i32 %1101, 8
  %1103 = and i32 %1102, 65280
  %1104 = or i32 %1103, %1093
  br label %1105

1105:                                             ; preds = %1086, %1090
  %1106 = phi i32 [ %1089, %1086 ], [ %1104, %1090 ]
  %1107 = lshr i32 %1106, 6
  br label %1135

1108:                                             ; preds = %1050
  %1109 = udiv i32 %1065, 3
  %1110 = shl i32 %1109, 2
  %1111 = add i32 %1064, %1110
  %1112 = and i32 %1111, 3
  %1113 = icmp eq i32 %1112, 0
  %1114 = lshr i32 %1111, 2
  %1115 = zext i32 %1114 to i64
  %1116 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %1115
  %1117 = load i32, i32 addrspace(1)* %1116, align 4, !tbaa !39
  br i1 %1113, label %1129, label %1118

1118:                                             ; preds = %1108
  %1119 = add nuw nsw i32 %1114, 1
  %1120 = zext i32 %1119 to i64
  %1121 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %1120
  %1122 = load i32, i32 addrspace(1)* %1121, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %1123 = shl nuw nsw i32 %1112, 3
  %1124 = lshr i32 %1117, %1123
  %1125 = sub nsw i32 0, %1123
  %1126 = and i32 %1125, 24
  %1127 = shl i32 %1122, %1126
  %1128 = or i32 %1127, %1124
  br label %1129

1129:                                             ; preds = %1108, %1118
  %1130 = phi i32 [ %1128, %1118 ], [ %1117, %1108 ]
  %1131 = urem i32 %1065, 3
  %1132 = mul nuw nsw i32 %1131, 10
  %1133 = lshr i32 %1130, %1132
  %1134 = and i32 %1133, 1023
  br label %1135

1135:                                             ; preds = %1129, %1105, %1066
  %1136 = phi i32 [ %1075, %1066 ], [ %1107, %1105 ], [ %1134, %1129 ]
  %1137 = insertelement <2 x i32> undef, i32 %1136, i64 0
  %1138 = or i32 %1065, 1
  switch i32 %17, label %1181 [
    i32 0, label %1139
    i32 1, label %1149
  ]

1139:                                             ; preds = %1135
  %1140 = add i32 %1064, %1138
  %1141 = lshr i32 %1140, 2
  %1142 = zext i32 %1141 to i64
  %1143 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %1142
  %1144 = load i32, i32 addrspace(1)* %1143, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %1145 = shl i32 %1140, 3
  %1146 = and i32 %1145, 24
  %1147 = lshr i32 %1144, %1146
  %1148 = and i32 %1147, 255
  br label %1208

1149:                                             ; preds = %1135
  %1150 = shl i32 %1138, 1
  %1151 = add i32 %1064, %1150
  %1152 = and i32 %1151, 1
  %1153 = icmp eq i32 %1152, 0
  %1154 = lshr i32 %1151, 2
  %1155 = zext i32 %1154 to i64
  %1156 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %1155
  %1157 = load i32, i32 addrspace(1)* %1156, align 4, !tbaa !39
  %1158 = shl i32 %1151, 3
  br i1 %1153, label %1159, label %1163

1159:                                             ; preds = %1149
  %1160 = and i32 %1158, 16
  %1161 = lshr i32 %1157, %1160
  %1162 = and i32 %1161, 65535
  br label %1178

1163:                                             ; preds = %1149
  %1164 = and i32 %1158, 24
  %1165 = lshr i32 %1157, %1164
  %1166 = and i32 %1165, 255
  %1167 = add i32 %1151, 1
  %1168 = lshr i32 %1167, 2
  %1169 = zext i32 %1168 to i64
  %1170 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %1169
  %1171 = load i32, i32 addrspace(1)* %1170, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %1172 = shl i32 %1167, 3
  %1173 = and i32 %1172, 24
  %1174 = lshr i32 %1171, %1173
  %1175 = shl i32 %1174, 8
  %1176 = and i32 %1175, 65280
  %1177 = or i32 %1176, %1166
  br label %1178

1178:                                             ; preds = %1159, %1163
  %1179 = phi i32 [ %1162, %1159 ], [ %1177, %1163 ]
  %1180 = lshr i32 %1179, 6
  br label %1208

1181:                                             ; preds = %1135
  %1182 = udiv i32 %1138, 3
  %1183 = shl i32 %1182, 2
  %1184 = add i32 %1064, %1183
  %1185 = and i32 %1184, 3
  %1186 = icmp eq i32 %1185, 0
  %1187 = lshr i32 %1184, 2
  %1188 = zext i32 %1187 to i64
  %1189 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %1188
  %1190 = load i32, i32 addrspace(1)* %1189, align 4, !tbaa !39
  br i1 %1186, label %1202, label %1191

1191:                                             ; preds = %1181
  %1192 = add nuw nsw i32 %1187, 1
  %1193 = zext i32 %1192 to i64
  %1194 = getelementptr inbounds i32, i32 addrspace(1)* %0, i64 %1193
  %1195 = load i32, i32 addrspace(1)* %1194, align 4, !tbaa !39, !alias.scope !40, !noalias !41
  %1196 = shl nuw nsw i32 %1185, 3
  %1197 = lshr i32 %1190, %1196
  %1198 = sub nsw i32 0, %1196
  %1199 = and i32 %1198, 24
  %1200 = shl i32 %1195, %1199
  %1201 = or i32 %1200, %1197
  br label %1202

1202:                                             ; preds = %1181, %1191
  %1203 = phi i32 [ %1201, %1191 ], [ %1190, %1181 ]
  %1204 = urem i32 %1138, 3
  %1205 = mul nuw nsw i32 %1204, 10
  %1206 = lshr i32 %1203, %1205
  %1207 = and i32 %1206, 1023
  br label %1208

1208:                                             ; preds = %1139, %1178, %1202
  %1209 = phi i32 [ %1148, %1139 ], [ %1180, %1178 ], [ %1207, %1202 ]
  %1210 = insertelement <2 x i32> %1137, i32 %1209, i64 1
  br label %1211

1211:                                             ; preds = %1043, %1208, %717
  %1212 = phi <2 x i32> [ %726, %717 ], [ %1049, %1043 ], [ %1210, %1208 ]
  %1213 = icmp eq i32 %17, 0
  %1214 = select i1 %1213, i32 8, i32 10
  %1215 = shl nsw i32 -1, %1214
  %1216 = xor i32 %1215, -1
  %1217 = icmp eq i32 %21, 2
  br i1 %1217, label %1218, label %1232

1218:                                             ; preds = %1211
  %1219 = tail call fast float @air.convert.f.f32.u.i32(i32 %1216) #3
  %1220 = fdiv fast float 1.000000e+00, %1219
  %1221 = extractelement <2 x i32> %1212, i64 1
  %1222 = tail call fast float @air.convert.f.f32.u.i32(i32 %1221) #3
  %1223 = fmul fast float %1222, %1220
  %1224 = insertelement <3 x float> undef, float %1223, i64 0
  %1225 = tail call fast float @air.convert.f.f32.u.i32(i32 %98) #3
  %1226 = fmul fast float %1225, %1220
  %1227 = insertelement <3 x float> %1224, float %1226, i64 1
  %1228 = extractelement <2 x i32> %1212, i64 0
  %1229 = tail call fast float @air.convert.f.f32.u.i32(i32 %1228) #3
  %1230 = fmul fast float %1229, %1220
  %1231 = insertelement <3 x float> %1227, float %1230, i64 2
  br label %1472

1232:                                             ; preds = %1211
  %1233 = sub nuw nsw i32 10, %1214
  %1234 = and i32 %16, 16
  %1235 = icmp eq i32 %1234, 0
  br i1 %1235, label %1236, label %1240

1236:                                             ; preds = %1232
  %1237 = shl nuw nsw i32 %98, %1233
  %1238 = mul i32 %1237, 9576
  %1239 = add i32 %1238, -612864
  br label %1246

1240:                                             ; preds = %1232
  %1241 = shl i32 %98, 15
  %1242 = lshr i32 %1216, 1
  %1243 = or i32 %1241, %1242
  %1244 = udiv i32 %1243, %1216
  %1245 = shl nsw i32 %1244, 8
  br label %1246

1246:                                             ; preds = %1240, %1236
  %1247 = phi i32 [ %1239, %1236 ], [ %1245, %1240 ]
  %1248 = extractelement <2 x i32> %1212, i64 0
  %1249 = shl i32 %1248, %1233
  %1250 = add nsw i32 %1249, -512
  %1251 = extractelement <2 x i32> %1212, i64 1
  %1252 = shl i32 %1251, %1233
  %1253 = add nsw i32 %1252, -512
  %1254 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 9
  %1255 = load i32, i32 addrspace(2)* %1254, align 4, !tbaa !46, !alias.scope !29, !noalias !32
  %1256 = mul nsw i32 %1255, %1250
  %1257 = add nsw i32 %1256, %1247
  %1258 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 10
  %1259 = load i32, i32 addrspace(2)* %1258, align 4, !tbaa !47, !alias.scope !29, !noalias !32
  %1260 = mul nsw i32 %1259, %1253
  %1261 = add nsw i32 %1257, %1260
  %1262 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 11
  %1263 = load i32, i32 addrspace(2)* %1262, align 4, !tbaa !48, !alias.scope !29, !noalias !32
  %1264 = mul nsw i32 %1263, %1250
  %1265 = add nsw i32 %1264, %1247
  %1266 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 12
  %1267 = load i32, i32 addrspace(2)* %1266, align 4, !tbaa !49, !alias.scope !29, !noalias !32
  %1268 = mul nsw i32 %1267, %1253
  %1269 = add nsw i32 %1265, %1268
  %1270 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 13
  %1271 = load i32, i32 addrspace(2)* %1270, align 4, !tbaa !50, !alias.scope !29, !noalias !32
  %1272 = mul nsw i32 %1271, %1250
  %1273 = add nsw i32 %1272, %1247
  %1274 = getelementptr inbounds %struct.YCbCrParams, %struct.YCbCrParams addrspace(2)* %1, i64 0, i32 14
  %1275 = load i32, i32 addrspace(2)* %1274, align 4, !tbaa !51, !alias.scope !29, !noalias !32
  %1276 = mul nsw i32 %1275, %1253
  %1277 = add nsw i32 %1273, %1276
  switch i32 %21, label %1417 [
    i32 0, label %1278
    i32 1, label %1297
    i32 3, label %1343
    i32 4, label %1359
  ]

1278:                                             ; preds = %1246
  %1279 = add nsw i32 %1261, 2048
  %1280 = ashr i32 %1279, 12
  %1281 = tail call i32 @air.clamp.s.i32(i32 %1280, i32 0, i32 2048) #3
  %1282 = tail call fast float @air.convert.f.f32.s.i32(i32 %1281) #3
  %1283 = fmul fast float %1282, 0x3F40000000000000
  %1284 = insertelement <3 x float> undef, float %1283, i64 0
  %1285 = add nsw i32 %1269, 2048
  %1286 = ashr i32 %1285, 12
  %1287 = tail call i32 @air.clamp.s.i32(i32 %1286, i32 0, i32 2048) #3
  %1288 = tail call fast float @air.convert.f.f32.s.i32(i32 %1287) #3
  %1289 = fmul fast float %1288, 0x3F40000000000000
  %1290 = insertelement <3 x float> %1284, float %1289, i64 1
  %1291 = add nsw i32 %1277, 2048
  %1292 = ashr i32 %1291, 12
  %1293 = tail call i32 @air.clamp.s.i32(i32 %1292, i32 0, i32 2048) #3
  %1294 = tail call fast float @air.convert.f.f32.s.i32(i32 %1293) #3
  %1295 = fmul fast float %1294, 0x3F40000000000000
  %1296 = insertelement <3 x float> %1290, float %1295, i64 2
  br label %1472

1297:                                             ; preds = %1246
  %1298 = add nsw i32 %1261, 512
  %1299 = ashr i32 %1298, 10
  %1300 = tail call i32 @air.clamp.s.i32(i32 %1299, i32 0, i32 8192) #3
  %1301 = tail call fast float @air.convert.f.f32.s.i32(i32 %1300) #3
  %1302 = fmul fast float %1301, 0x3F20000000000000
  %1303 = add nsw i32 %1269, 512
  %1304 = ashr i32 %1303, 10
  %1305 = tail call i32 @air.clamp.s.i32(i32 %1304, i32 0, i32 8192) #3
  %1306 = tail call fast float @air.convert.f.f32.s.i32(i32 %1305) #3
  %1307 = fmul fast float %1306, 0x3F20000000000000
  %1308 = add nsw i32 %1277, 512
  %1309 = ashr i32 %1308, 10
  %1310 = tail call i32 @air.clamp.s.i32(i32 %1309, i32 0, i32 8192) #3
  %1311 = tail call fast float @air.convert.f.f32.s.i32(i32 %1310) #3
  %1312 = fmul fast float %1311, 0x3F20000000000000
  %1313 = fcmp fast ugt float %1302, 0x3FA4B5DCC0000000
  br i1 %1313, label %1316, label %1314

1314:                                             ; preds = %1297
  %1315 = fmul fast float %1301, 0x3EE3D07220000000
  br label %1320

1316:                                             ; preds = %1297
  %1317 = fmul fast float %1301, 0x3F1E54EDE0000000
  %1318 = fadd fast float %1317, 0x3FAAB12340000000
  %1319 = tail call fast float @air.fast_pow.f32(float %1318, float 0x4003333340000000) #3
  br label %1320

1320:                                             ; preds = %1314, %1316
  %1321 = phi fast float [ %1315, %1314 ], [ %1319, %1316 ]
  %1322 = insertelement <3 x float> undef, float %1321, i64 0
  %1323 = fcmp fast ugt float %1307, 0x3FA4B5DCC0000000
  br i1 %1323, label %1326, label %1324

1324:                                             ; preds = %1320
  %1325 = fmul fast float %1306, 0x3EE3D07220000000
  br label %1330

1326:                                             ; preds = %1320
  %1327 = fmul fast float %1306, 0x3F1E54EDE0000000
  %1328 = fadd fast float %1327, 0x3FAAB12340000000
  %1329 = tail call fast float @air.fast_pow.f32(float %1328, float 0x4003333340000000) #3
  br label %1330

1330:                                             ; preds = %1324, %1326
  %1331 = phi fast float [ %1325, %1324 ], [ %1329, %1326 ]
  %1332 = insertelement <3 x float> %1322, float %1331, i64 1
  %1333 = fcmp fast ugt float %1312, 0x3FA4B5DCC0000000
  br i1 %1333, label %1336, label %1334

1334:                                             ; preds = %1330
  %1335 = fmul fast float %1311, 0x3EE3D07220000000
  br label %1340

1336:                                             ; preds = %1330
  %1337 = fmul fast float %1311, 0x3F1E54EDE0000000
  %1338 = fadd fast float %1337, 0x3FAAB12340000000
  %1339 = tail call fast float @air.fast_pow.f32(float %1338, float 0x4003333340000000) #3
  br label %1340

1340:                                             ; preds = %1334, %1336
  %1341 = phi fast float [ %1335, %1334 ], [ %1339, %1336 ]
  %1342 = insertelement <3 x float> %1332, float %1341, i64 2
  br label %1472

1343:                                             ; preds = %1246
  %1344 = add nsw i32 %1261, 2048
  %1345 = ashr i32 %1344, 12
  %1346 = tail call fast float @air.convert.f.f32.s.i32(i32 %1345) #3
  %1347 = fmul fast float %1346, 0x3F40000000000000
  %1348 = insertelement <3 x float> undef, float %1347, i64 0
  %1349 = add nsw i32 %1269, 2048
  %1350 = ashr i32 %1349, 12
  %1351 = tail call fast float @air.convert.f.f32.s.i32(i32 %1350) #3
  %1352 = fmul fast float %1351, 0x3F40000000000000
  %1353 = insertelement <3 x float> %1348, float %1352, i64 1
  %1354 = add nsw i32 %1277, 2048
  %1355 = ashr i32 %1354, 12
  %1356 = tail call fast float @air.convert.f.f32.s.i32(i32 %1355) #3
  %1357 = fmul fast float %1356, 0x3F40000000000000
  %1358 = insertelement <3 x float> %1353, float %1357, i64 2
  br label %1472

1359:                                             ; preds = %1246
  %1360 = add nsw i32 %1261, 512
  %1361 = ashr i32 %1360, 10
  %1362 = tail call i32 @air.clamp.s.i32(i32 %1361, i32 0, i32 8192) #3
  %1363 = tail call fast float @air.convert.f.f32.s.i32(i32 %1362) #3
  %1364 = fmul fast float %1363, 0x3F20000000000000
  %1365 = add nsw i32 %1269, 512
  %1366 = ashr i32 %1365, 10
  %1367 = tail call i32 @air.clamp.s.i32(i32 %1366, i32 0, i32 8192) #3
  %1368 = tail call fast float @air.convert.f.f32.s.i32(i32 %1367) #3
  %1369 = fmul fast float %1368, 0x3F20000000000000
  %1370 = add nsw i32 %1277, 512
  %1371 = ashr i32 %1370, 10
  %1372 = tail call i32 @air.clamp.s.i32(i32 %1371, i32 0, i32 8192) #3
  %1373 = tail call fast float @air.convert.f.f32.s.i32(i32 %1372) #3
  %1374 = fmul fast float %1373, 0x3F20000000000000
  %1375 = fcmp fast ugt float %1364, 0.000000e+00
  br i1 %1375, label %1376, label %1386

1376:                                             ; preds = %1359
  %1377 = tail call fast float @air.fast_pow.f32(float %1364, float 0x3F89F9B580000000) #3
  %1378 = fadd fast float %1377, 0xBFEAC00000000000
  %1379 = fcmp fast ugt float %1378, 0.000000e+00
  br i1 %1379, label %1380, label %1386

1380:                                             ; preds = %1376
  %1381 = fmul fast float %1377, 1.868750e+01
  %1382 = fsub fast float 0x4032DA0000000000, %1381
  %1383 = tail call fast float @air.fast_fmax.f32(float %1382, float 0x3EB0C6F7A0000000) #3
  %1384 = fdiv fast float %1378, %1383
  %1385 = tail call fast float @air.fast_pow.f32(float %1384, float 0x40191C0D60000000) #3
  br label %1386

1386:                                             ; preds = %1380, %1376, %1359
  %1387 = phi float [ 0.000000e+00, %1359 ], [ %1385, %1380 ], [ 0.000000e+00, %1376 ]
  %1388 = insertelement <3 x float> undef, float %1387, i64 0
  %1389 = fcmp fast ugt float %1369, 0.000000e+00
  br i1 %1389, label %1390, label %1400

1390:                                             ; preds = %1386
  %1391 = tail call fast float @air.fast_pow.f32(float %1369, float 0x3F89F9B580000000) #3
  %1392 = fadd fast float %1391, 0xBFEAC00000000000
  %1393 = fcmp fast ugt float %1392, 0.000000e+00
  br i1 %1393, label %1394, label %1400

1394:                                             ; preds = %1390
  %1395 = fmul fast float %1391, 1.868750e+01
  %1396 = fsub fast float 0x4032DA0000000000, %1395
  %1397 = tail call fast float @air.fast_fmax.f32(float %1396, float 0x3EB0C6F7A0000000) #3
  %1398 = fdiv fast float %1392, %1397
  %1399 = tail call fast float @air.fast_pow.f32(float %1398, float 0x40191C0D60000000) #3
  br label %1400

1400:                                             ; preds = %1394, %1390, %1386
  %1401 = phi float [ 0.000000e+00, %1386 ], [ %1399, %1394 ], [ 0.000000e+00, %1390 ]
  %1402 = insertelement <3 x float> %1388, float %1401, i64 1
  %1403 = fcmp fast ugt float %1374, 0.000000e+00
  br i1 %1403, label %1404, label %1414

1404:                                             ; preds = %1400
  %1405 = tail call fast float @air.fast_pow.f32(float %1374, float 0x3F89F9B580000000) #3
  %1406 = fadd fast float %1405, 0xBFEAC00000000000
  %1407 = fcmp fast ugt float %1406, 0.000000e+00
  br i1 %1407, label %1408, label %1414

1408:                                             ; preds = %1404
  %1409 = fmul fast float %1405, 1.868750e+01
  %1410 = fsub fast float 0x4032DA0000000000, %1409
  %1411 = tail call fast float @air.fast_fmax.f32(float %1410, float 0x3EB0C6F7A0000000) #3
  %1412 = fdiv fast float %1406, %1411
  %1413 = tail call fast float @air.fast_pow.f32(float %1412, float 0x40191C0D60000000) #3
  br label %1414

1414:                                             ; preds = %1408, %1404, %1400
  %1415 = phi float [ 0.000000e+00, %1400 ], [ %1413, %1408 ], [ 0.000000e+00, %1404 ]
  %1416 = insertelement <3 x float> %1402, float %1415, i64 2
  br label %1472

1417:                                             ; preds = %1246
  %1418 = add nsw i32 %1261, 512
  %1419 = ashr i32 %1418, 10
  %1420 = tail call i32 @air.clamp.s.i32(i32 %1419, i32 0, i32 8192) #3
  %1421 = tail call fast float @air.convert.f.f32.s.i32(i32 %1420) #3
  %1422 = fmul fast float %1421, 0x3F20000000000000
  %1423 = add nsw i32 %1269, 512
  %1424 = ashr i32 %1423, 10
  %1425 = tail call i32 @air.clamp.s.i32(i32 %1424, i32 0, i32 8192) #3
  %1426 = tail call fast float @air.convert.f.f32.s.i32(i32 %1425) #3
  %1427 = fmul fast float %1426, 0x3F20000000000000
  %1428 = add nsw i32 %1277, 512
  %1429 = ashr i32 %1428, 10
  %1430 = tail call i32 @air.clamp.s.i32(i32 %1429, i32 0, i32 8192) #3
  %1431 = tail call fast float @air.convert.f.f32.s.i32(i32 %1430) #3
  %1432 = fmul fast float %1431, 0x3F20000000000000
  %1433 = fcmp fast ugt float %1422, 5.000000e-01
  br i1 %1433, label %1437, label %1434

1434:                                             ; preds = %1417
  %1435 = fmul fast float %1421, 0x3F05555560000000
  %1436 = fmul fast float %1435, %1422
  br label %1443

1437:                                             ; preds = %1417
  %1438 = fmul fast float %1421, 0x3F465E0520000000
  %1439 = fadd fast float %1438, 0xC0090C1EC0000000
  %1440 = tail call fast float @air.fast_exp.f32(float %1439) #3
  %1441 = fadd fast float %1440, 0x3FD2380400000000
  %1442 = fmul fast float %1441, 0x3FB5555560000000
  br label %1443

1443:                                             ; preds = %1434, %1437
  %1444 = phi float [ %1436, %1434 ], [ %1442, %1437 ]
  %1445 = insertelement <3 x float> undef, float %1444, i64 0
  %1446 = fcmp fast ugt float %1427, 5.000000e-01
  br i1 %1446, label %1450, label %1447

1447:                                             ; preds = %1443
  %1448 = fmul fast float %1426, 0x3F05555560000000
  %1449 = fmul fast float %1448, %1427
  br label %1456

1450:                                             ; preds = %1443
  %1451 = fmul fast float %1426, 0x3F465E0520000000
  %1452 = fadd fast float %1451, 0xC0090C1EC0000000
  %1453 = tail call fast float @air.fast_exp.f32(float %1452) #3
  %1454 = fadd fast float %1453, 0x3FD2380400000000
  %1455 = fmul fast float %1454, 0x3FB5555560000000
  br label %1456

1456:                                             ; preds = %1447, %1450
  %1457 = phi float [ %1449, %1447 ], [ %1455, %1450 ]
  %1458 = insertelement <3 x float> %1445, float %1457, i64 1
  %1459 = fcmp fast ugt float %1432, 5.000000e-01
  br i1 %1459, label %1463, label %1460

1460:                                             ; preds = %1456
  %1461 = fmul fast float %1431, 0x3F05555560000000
  %1462 = fmul fast float %1461, %1432
  br label %1469

1463:                                             ; preds = %1456
  %1464 = fmul fast float %1431, 0x3F465E0520000000
  %1465 = fadd fast float %1464, 0xC0090C1EC0000000
  %1466 = tail call fast float @air.fast_exp.f32(float %1465) #3
  %1467 = fadd fast float %1466, 0x3FD2380400000000
  %1468 = fmul fast float %1467, 0x3FB5555560000000
  br label %1469

1469:                                             ; preds = %1460, %1463
  %1470 = phi float [ %1462, %1460 ], [ %1468, %1463 ]
  %1471 = insertelement <3 x float> %1458, float %1470, i64 2
  br label %1472

1472:                                             ; preds = %1278, %1343, %1469, %1414, %1340, %1218
  %1473 = phi <3 x float> [ %1231, %1218 ], [ %1296, %1278 ], [ %1342, %1340 ], [ %1358, %1343 ], [ %1416, %1414 ], [ %1471, %1469 ]
  %1474 = shufflevector <3 x float> %1473, <3 x float> poison, <4 x i32> <i32 0, i32 1, i32 2, i32 undef>
  %1475 = insertelement <4 x float> %1474, float 1.000000e+00, i64 3
  tail call void @air.write_texture_2d.v4f32(%struct._texture_2d_t addrspace(1)* nocapture %2, <2 x i32> %3, <4 x float> %1475, i32 0, i32 2) #4, !alias.scope !52, !noalias !53
  br label %1476

1476:                                             ; preds = %4, %9, %1472
  ret void
}

; Function Attrs: mustprogress nofree nosync nounwind readnone willreturn
declare float @air.convert.f.f32.u.i32(i32) local_unnamed_addr #1

; Function Attrs: mustprogress nofree nosync nounwind readnone willreturn
declare float @air.convert.f.f32.s.i32(i32) local_unnamed_addr #1

; Function Attrs: mustprogress nofree nosync nounwind readnone willreturn
declare i32 @air.min.u.i32(i32, i32) local_unnamed_addr #1

; Function Attrs: mustprogress nofree nosync nounwind readnone willreturn
declare i32 @air.clamp.s.i32(i32, i32, i32) local_unnamed_addr #1

; Function Attrs: mustprogress nofree nosync nounwind readnone willreturn
declare float @air.fast_pow.f32(float, float) local_unnamed_addr #1

; Function Attrs: mustprogress nofree nosync nounwind readnone willreturn
declare float @air.fast_fmax.f32(float, float) local_unnamed_addr #1

; Function Attrs: mustprogress nofree nosync nounwind readnone willreturn
declare float @air.fast_exp.f32(float) local_unnamed_addr #1

; Function Attrs: argmemonly mustprogress nounwind willreturn
declare void @air.write_texture_2d.v4f32(%struct._texture_2d_t addrspace(1)* nocapture, <2 x i32>, <4 x float>, i32, i32) local_unnamed_addr #2

attributes #0 = { mustprogress nounwind willreturn "approx-func-fp-math"="true" "frame-pointer"="all" "min-legal-vector-width"="128" "no-builtins" "no-infs-fp-math"="true" "no-nans-fp-math"="true" "no-signed-zeros-fp-math"="true" "no-trapping-math"="true" "stack-protector-buffer-size"="8" "unsafe-fp-math"="true" }
attributes #1 = { mustprogress nofree nosync nounwind readnone willreturn }
attributes #2 = { argmemonly mustprogress nounwind willreturn }
attributes #3 = { nounwind readnone willreturn }
attributes #4 = { argmemonly nounwind willreturn }

!llvm.module.flags = !{!0, !1, !2, !3, !4, !5, !6, !7, !8}
!air.kernel = !{!9}
!air.compile_options = !{!17, !18, !19}
!llvm.ident = !{!20}
!air.version = !{!21}
!air.language_version = !{!22}
!air.source_file_name = !{!23}

!0 = !{i32 2, !"SDK Version", [2 x i32] [i32 26, i32 5]}
!1 = !{i32 1, !"wchar_size", i32 4}
!2 = !{i32 7, !"frame-pointer", i32 2}
!3 = !{i32 7, !"air.max_device_buffers", i32 31}
!4 = !{i32 7, !"air.max_constant_buffers", i32 31}
!5 = !{i32 7, !"air.max_threadgroup_buffers", i32 31}
!6 = !{i32 7, !"air.max_textures", i32 128}
!7 = !{i32 7, !"air.max_read_write_textures", i32 8}
!8 = !{i32 7, !"air.max_samplers", i32 16}
!9 = !{void (i32 addrspace(1)*, %struct.YCbCrParams addrspace(2)*, %struct._texture_2d_t addrspace(1)*, <2 x i32>)* @ycbcr_convert, !10, !11}
!10 = !{}
!11 = !{!12, !13, !15, !16}
!12 = !{i32 0, !"air.buffer", !"air.location_index", i32 0, i32 1, !"air.read", !"air.address_space", i32 1, !"air.arg_type_size", i32 4, !"air.arg_type_align_size", i32 4, !"air.arg_type_name", !"uint", !"air.arg_name", !"src"}
!13 = !{i32 1, !"air.buffer", !"air.buffer_size", i32 64, !"air.location_index", i32 1, i32 1, !"air.read", !"air.address_space", i32 2, !"air.struct_type_info", !14, !"air.arg_type_size", i32 64, !"air.arg_type_align_size", i32 4, !"air.arg_type_name", !"YCbCrParams", !"air.arg_name", !"p"}
!14 = !{i32 0, i32 4, i32 0, !"uint", !"width", i32 4, i32 4, i32 0, !"uint", !"height", i32 8, i32 4, i32 0, !"uint", !"luma_offset", i32 12, i32 4, i32 0, !"uint", !"luma_row_bytes", i32 16, i32 4, i32 0, !"uint", !"chroma_offset", i32 20, i32 4, i32 0, !"uint", !"chroma_row_bytes", i32 24, i32 4, i32 0, !"uint", !"chroma_w", i32 28, i32 4, i32 0, !"uint", !"chroma_h", i32 32, i32 4, i32 0, !"uint", !"flags", i32 36, i32 4, i32 0, !"int", !"kb_r", i32 40, i32 4, i32 0, !"int", !"kr_r", i32 44, i32 4, i32 0, !"int", !"kb_g", i32 48, i32 4, i32 0, !"int", !"kr_g", i32 52, i32 4, i32 0, !"int", !"kb_b", i32 56, i32 4, i32 0, !"int", !"kr_b", i32 60, i32 4, i32 0, !"uint", !"_pad"}
!15 = !{i32 2, !"air.texture", !"air.location_index", i32 0, i32 1, !"air.write", !"air.arg_type_name", !"texture2d<float, write>", !"air.arg_name", !"dst"}
!16 = !{i32 3, !"air.thread_position_in_grid", !"air.arg_type_name", !"uint2", !"air.arg_name", !"gid"}
!17 = !{!"air.compile.denorms_disable"}
!18 = !{!"air.compile.fast_math_enable"}
!19 = !{!"air.compile.framebuffer_fetch_enable"}
!20 = !{!"Apple metal version 32023.883 (metalfe-32023.883)"}
!21 = !{i32 2, i32 8, i32 0}
!22 = !{!"Metal", i32 3, i32 1, i32 0}
!23 = !{!"ycbcr_convert.metal"}
!24 = !{!25, !26, i64 0}
!25 = !{!"_ZTS11YCbCrParams", !26, i64 0, !26, i64 4, !26, i64 8, !26, i64 12, !26, i64 16, !26, i64 20, !26, i64 24, !26, i64 28, !26, i64 32, !26, i64 36, !26, i64 40, !26, i64 44, !26, i64 48, !26, i64 52, !26, i64 56, !26, i64 60}
!26 = !{!"int", !27, i64 0}
!27 = !{!"omnipotent char", !28, i64 0}
!28 = !{!"Simple C++ TBAA"}
!29 = !{!30}
!30 = distinct !{!30, !31, !"air-alias-scope-arg(1)"}
!31 = distinct !{!31, !"air-alias-scopes(ycbcr_convert)"}
!32 = !{!33, !34}
!33 = distinct !{!33, !31, !"air-alias-scope-arg(0)"}
!34 = distinct !{!34, !31, !"air-alias-scope-textures"}
!35 = !{!25, !26, i64 4}
!36 = !{!25, !26, i64 32}
!37 = !{!25, !26, i64 8}
!38 = !{!25, !26, i64 12}
!39 = !{!26, !26, i64 0}
!40 = !{!33}
!41 = !{!30, !34}
!42 = !{!25, !26, i64 24}
!43 = !{!25, !26, i64 28}
!44 = !{!25, !26, i64 16}
!45 = !{!25, !26, i64 20}
!46 = !{!25, !26, i64 36}
!47 = !{!25, !26, i64 40}
!48 = !{!25, !26, i64 44}
!49 = !{!25, !26, i64 48}
!50 = !{!25, !26, i64 52}
!51 = !{!25, !26, i64 56}
!52 = !{!34}
!53 = !{!33, !30}
