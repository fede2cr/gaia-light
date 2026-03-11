# Gaia Light — Models

Computer-vision models used or planned for the gaia-light camera-trap
pipeline.  The processing flow is:

```
Frame  →  MegaDetector (detection)
              ↓ crop
          Species classifier (species ID)
              ↓ if person
          OSNet re-ID (individual ID)
```

---

## Ready to Use

These models are baked into the processing container image at build time.
The Containerfile `converter` stage exports them to ONNX via
`processing/scripts/export_models.py`.  On first start the baked-in files
are seeded into the `/models` volume.

### MegaDetector v6 (Detection)

| Field | Value |
|-------|-------|
| **Purpose** | Object detection — animal / person / vehicle |
| **Architecture** | YOLOv5 |
| **Input** | `[1, 3, 640, 640]` (NCHW, RGB, normalised 0–1) |
| **Output** | `[1, N, 8]` (cx, cy, w, h, obj, cls0, cls1, cls2) |
| **Classes** | 0 = animal, 1 = person, 2 = vehicle |
| **Source** | [HuggingFace — ai-for-good-lab/megadetector-onnx](https://huggingface.co/ai-for-good-lab/megadetector-onnx) |
| **Slug** | `pytorch-wildlife` (via PytorchWildlife export) |
| **Status** | Fully integrated — baked into container image |

MegaDetector is the de-facto standard detector for camera-trap imagery.
The v6 weights are obtained via the PytorchWildlife library and exported
to ONNX at build time.  NMS (IoU 0.45) and confidence thresholding
(`CONFIDENCE` env var, default 0.5) are applied in Rust.

---

### Google SpeciesNet v4.0.1a (Species Classification)

| Field | Value |
|-------|-------|
| **Purpose** | Species-level classification from detection crops |
| **Architecture** | EfficientNet V2 M |
| **Input** | `[1, 3, 480, 480]` (NCHW, RGB, normalised 0–1) |
| **Output** | `[1, C]` logits (softmax applied in Rust) |
| **Species** | ~2,500 classes |
| **Source** | [HuggingFace — Addax-Data-Science/SPECIESNET-v4-0-1-A-v1](https://huggingface.co/Addax-Data-Science/SPECIESNET-v4-0-1-A-v1) |
| **Slug** | `speciesnet` |
| **Status** | Fully integrated — baked into container image |

Google's open-source camera-trap species classifier, covering a broad
global species list.  Accepts standard square crops and outputs softmax
probability over all species.  The ONNX model is converted from PyTorch
at build time (no public ONNX download URL exists).

---

### AI for Good — Amazon Rainforest V2 (Species Classification)

| Field | Value |
|-------|-------|
| **Purpose** | Species classification — Neotropical fauna |
| **Architecture** | CNN (via PytorchWildlife) |
| **Input** | `[1, 3, 224, 224]` (NCHW, RGB, normalised 0–1) |
| **Output** | `[1, C]` logits (softmax applied in Rust) |
| **Source** | [PytorchWildlife — AI4G Amazon Classifier](https://github.com/microsoft/CameraTraps/tree/main/PytorchWildlife) |
| **Slug** | `ai4g-amazon-v2` |
| **Status** | Fully integrated — baked into container image |

Microsoft AI for Good Lab's species classifier trained on Amazon
rainforest camera-trap imagery.  Narrower species coverage than
SpeciesNet but higher accuracy for Neotropical fauna.  When multiple
classifiers are enabled, the best-confidence result is kept.

---

---

## Individual Re-Identification (Animal)

Re-ID models take a **cropped detection image** and produce an embedding
vector.  Matching embeddings via cosine similarity identifies whether two
images show the same individual.  This is useful for population surveys,
mark-recapture studies, and tracking animals over time.

### MegaDescriptor (Foundation Model for Wildlife Re-ID)

| Field | Value |
|-------|-------|
| **Purpose** | Individual animal re-identification via embedding |
| **Architecture** | Swin Transformer (timm) |
| **Input** | `[1, 3, 384, 384]` (NCHW, RGB, normalised 0–1) |
| **Output** | `[1, 768]` or `[1, 1536]` embedding vector |
| **Species coverage** | Broad zero-shot: big cats, whales, elephants, turtles, zebras, primates, etc. |
| **Source** | [HuggingFace — WildlifeDatasets/MegaDescriptor-L-384](https://huggingface.co/WildlifeDatasets/MegaDescriptor-L-384) |
| **License** | MIT |
| **Paper** | "MegaDescriptor: Foundation Model for Wildlife Re-Identification" (2025) |
| **Status** | ONNX-exportable — needs integration |

The most practical drop-in model for wildlife re-ID.  Trained on 49
wildlife re-ID datasets spanning millions of images.  Zero-shot
performance across many species means it works **without fine-tuning** —
feed it a cropped animal image and get an embedding, then match against a
gallery of known individuals using cosine similarity.

Since it's a standard timm model, ONNX export is straightforward:

```python
import timm, torch

model = timm.create_model(
    "hf-hub:WildlifeDatasets/MegaDescriptor-L-384",
    num_classes=0,    # embedding mode
    pretrained=True,
)
model.eval()

dummy = torch.randn(1, 3, 384, 384)
torch.onnx.export(model, dummy, "megadescriptor.onnx",
                   input_names=["image"],
                   output_names=["embedding"],
                   dynamic_axes={"image": {0: "batch"},
                                 "embedding": {0: "batch"}})
```

**Integration path:** The existing pipeline already saves detection crops
(`crop_path` in the `detections` table).  A re-ID step would:

1. Load `megadescriptor.onnx` via tract-onnx (same pattern as classifiers)
2. Run inference on each crop → 768-d embedding
3. Store embedding in a new `individuals` table
4. Match against the gallery via cosine similarity
5. Assign an `individual_id` (or create a new one if no match above threshold)

**Important limitation:** Re-ID works best for species with individually
distinguishing markings — coat patterns (leopards, giraffes, zebras),
scars (whales), shell patterns (turtles), facial structure (primates).
For visually uniform species (most small rodents, songbirds), re-ID
accuracy is low.

---

### Other Wildlife Re-ID Approaches (Not Drop-in)

| System | Method | Best for | Source |
|--------|--------|----------|--------|
| **WBIA / Wildbook** (powers Flukebook) | HotSpotter (SIFT texture matching), PIE (Pose Invariant Embeddings), CurvRank (dorsal fin contours) | Cetaceans, giraffes, whale sharks | [GitHub — WildMeOrg/wildbook-ia](https://github.com/WildMeOrg/wildbook-ia) |
| **WildFIR** | DINOv2-based self-supervised embeddings | General wildlife, newer/less mature | Academic — no public weights |
| **YOLO + ArcFace** | Detection + metric learning head | Custom species with training data | Various implementations |

These are full Python systems or require training on target species — not
single ONNX models.  WBIA/Wildbook is the most mature but is designed as
a standalone platform, not an embeddable model.

---

---

## Individual Re-Identification (Human / Person)

MegaDetector already detects people (class 1 = "person").  The person
re-ID model lets gaia-light function as a security camera — tracking
known vs. unknown individuals across video segments.  Unknown persons
trigger a console warning (`⚠️ UNKNOWN PERSON detected`) that can be
used for security alerting.  The web dashboard has a **People** page
where each individual can be viewed and named.

### OSNet (Omni-Scale Network for Person Re-ID)

| Field | Value |
|-------|-------|
| **Purpose** | Person re-identification via embedding |
| **Architecture** | OSNet — multi-scale feature learning CNN |
| **Input** | `[1, 3, 256, 128]` (NCHW, RGB, normalised 0–1) |
| **Output** | `[1, 512]` embedding vector |
| **Training data** | Market-1501, DukeMTMC, MSMT17 |
| **Source** | [GitHub — KaiyangZhou/deep-person-reid (torchreid)](https://github.com/KaiyangZhou/deep-person-reid) |
| **Weights** | [Pre-trained weights catalog](https://kaiyangzhou.github.io/deep-person-reid/MODEL_ZOO) |
| **Status** | **Integrated** — baked into container image, person re-ID active |
| **License** | MIT |
| **Status** | ONNX-exportable — needs integration |

The most widely used lightweight person re-ID model.  Specifically
designed for re-identifying the same person across different camera views
and time.  Produces a 512-d embedding per person crop.

Variants by size vs. accuracy trade-off:

| Variant | Parameters | mAP (Market-1501) |
|---------|-----------|-------------------|
| `osnet_x1_0` | 2.2 M | 84.9% |
| `osnet_x0_75` | 1.3 M | 83.4% |
| `osnet_x0_5` | 0.6 M | 82.6% |
| `osnet_x0_25` | 0.2 M | 77.5% |
| `osnet_ain_x1_0` | 2.2 M | 87.2% (domain generalisation variant) |

ONNX export:

```python
import torch
from torchreid.utils import FeatureExtractor

extractor = FeatureExtractor(
    model_name="osnet_ain_x1_0",
    model_path="osnet_ain_x1_0_msmt17.pth",
    device="cpu",
)
model = extractor.model.eval()

dummy = torch.randn(1, 3, 256, 128)
torch.onnx.export(model, dummy, "osnet_ain.onnx",
                   input_names=["image"],
                   output_names=["embedding"],
                   dynamic_axes={"image": {0: "batch"},
                                 "embedding": {0: "batch"}})
```

---

### TransReID (Transformer-based Person Re-ID)

| Field | Value |
|-------|-------|
| **Purpose** | Person re-identification — higher accuracy |
| **Architecture** | ViT-Base/S with side information embeddings |
| **Input** | `[1, 3, 256, 128]` (NCHW, RGB, normalised 0–1) |
| **Output** | `[1, 768]` embedding vector |
| **Training data** | Market-1501, MSMT17 |
| **Source** | [GitHub — damo-cv/TransReID](https://github.com/damo-cv/TransReID) |
| **License** | MIT |
| **Status** | ONNX-exportable — needs integration |

Higher accuracy than OSNet but heavier (ViT-Base: ~86 M parameters).
Better suited when running on machines with a GPU or when accuracy is
prioritised over speed.

---

### Other Person Re-ID Approaches

| Model | Architecture | Parameters | Notes |
|-------|-------------|-----------|-------|
| **FastReID** (Meta) | Multi-backbone framework | Varies | [GitHub — JDAI-CV/fast-reid](https://github.com/JDAI-CV/fast-reid) — flexible, supports BOT, AGW, SBS |
| **BoT (Bag of Tricks)** | ResNet-50 + tricks | 23 M | Solid baseline, easy ONNX export |
| **CLIP-ReID** | CLIP ViT-B/16 fine-tuned | 86 M | Uses text-image pretraining, state-of-the-art on MSMT17 |

---

---

## Integration Roadmap for Re-ID

Adding re-ID (animal or person) to gaia-light would follow this pattern:

### Database changes

```sql
-- Embedding gallery for known individuals
CREATE TABLE individuals (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT NOT NULL DEFAULT '',
    embedding       BLOB NOT NULL,   -- 512-d float32 vector (little-endian)
    detection_count INTEGER NOT NULL DEFAULT 1,
    first_seen      TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen       TEXT NOT NULL DEFAULT (datetime('now')),
    representative_crop TEXT
);

-- Link detections to individuals
ALTER TABLE detections ADD COLUMN individual_id INTEGER
    REFERENCES individuals(id);
```

### Pipeline (implemented)

1. `ReIdentifier` model struct loads `osnet_ain.onnx` via tract-onnx
2. After species classification, if detection class is `"person"`,
   the crop is passed to `ReIdentifier::embed()` to get a 512-d vector
3. L2-normalised embedding is compared via cosine similarity against
   all rows in the `individuals` table
4. If best match > threshold (0.65) → assign `individual_id`, bump count
5. If no match → create new individual + log
   `⚠️ UNKNOWN PERSON detected — created new individual #N`

### Web dashboard

The **People** page (`/people`) lists all known individuals with their
representative crop, sighting count, and last-seen timestamp.  Users can
rename individuals via the inline form.  Detection cards also show the
individual identity for person detections.

### Recommended first model

**OSNet `osnet_ain_x1_0`** for humans (2.2 M params, fast, MIT license)
— **currently integrated**.  **MegaDescriptor-L-384** for animals
(Swin-L, broader zero-shot coverage) can be added in the future.

### Configuration

| Key | Default | Description |
|-----|---------|-------------|
| `REID_MODELS` | _(none)_ | Comma-separated: `megadescriptor`, `osnet` |
| `REID_THRESHOLD` | `0.7` | Cosine similarity threshold for match |
| `REID_ENABLED_CLASSES` | `animal,person` | Which MegaDetector classes to run re-ID on |

---

## Integration Checklist

For any new model:

1. **ONNX model file** — export from PyTorch to `.onnx`
2. **Labels file** — (for classifiers) one label per line
3. **Add a variant** to `ClassifierKind` in `common/src/classifier_kind.rs`
   (or create a new `ReIdKind` enum for re-ID models)
4. **Add download/bake logic** to `download.rs` and Containerfile
5. **Build-time smoke test** — `--check-models` flag validates tract-onnx
   loading
6. Verify input dimensions match the ONNX graph
7. Set `apply_softmax` appropriately (classifiers only)

### Container deployment

Models are baked into the container image during the `converter` stage via
`processing/scripts/export_models.py`.  On first start they are seeded
from `/usr/local/share/gaia/models/` into the `/models` volume (same
pattern as gaia-audio).

### Adding a new model

1. Add export logic to `processing/scripts/export_models.py`
2. If it's a classifier, add a `ClassifierKind` variant
3. If it's a re-ID model, create a `ReIdKind` enum + `ReIdentifier` struct
4. Add bake path to Containerfile: `COPY --from=converter /export/models/ ...`
5. Rebuild the processing container image
