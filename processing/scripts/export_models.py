#!/usr/bin/env python3
"""Export MegaDetector v5a and SpeciesNet v4.0.1a to ONNX.

Called during container image build (see Containerfile, converter stage).

Usage:
    python export_models.py /output/dir

Produces:
    /output/dir/megadetector_v6.onnx      (YOLOv5-based detector)
    /output/dir/speciesnet.onnx            (EfficientNet V2 M species classifier)
    /output/dir/speciesnet_labels.txt      (species label list, ~2000+ classes)

Sources:
    MegaDetector v5a    — agentmorris/MegaDetector on GitHub (MIT)
    SpeciesNet v4.0.1a  — Addax-Data-Science/SPECIESNET-v4-0-1-A-v1
                          on HuggingFace (Apache 2.0, public, no auth)
"""

import os
import sys
import traceback
import urllib.request


def export_megadetector(output_dir: str) -> None:
    """Download MegaDetector v5a weights and export to ONNX.

    Despite our file being named megadetector_v6.onnx, we use the v5a
    weights which are YOLOv5-based and match our Rust inference code's
    expected output format: [1, N, 8] (cx, cy, w, h, obj, cls0, cls1, cls2).

    MegaDetector v6 uses YOLOv9/v10 architectures with different output
    formats.  We keep the v6 filename for forward compatibility — when
    actual v6 ONNX exports become standard, they can be swapped in.

    We use ``torch.hub.load('ultralytics/yolov5', 'custom', ...)`` to
    load the checkpoint because it clones the original YOLOv5 repo and
    makes the ``models.yolo`` module available — the modern
    ``ultralytics`` package no longer includes it.
    """
    import numpy as np
    import onnx
    import onnxruntime as ort
    import torch
    from onnxsim import simplify as onnx_simplify

    # MegaDetector v5a weights — hosted on the agentmorris fork
    weight_url = (
        "https://github.com/agentmorris/MegaDetector/releases/download/"
        "v5.0/md_v5a.0.0.pt"
    )
    weight_path = os.path.join(output_dir, "md_v5a.0.0.pt")
    onnx_path = os.path.join(output_dir, "megadetector_v6.onnx")

    # -- 1. Download weights ------------------------------------------------
    if not os.path.exists(weight_path):
        print(f"Downloading MegaDetector v5a weights → {weight_path}")
        urllib.request.urlretrieve(weight_url, weight_path)
        print(f"Downloaded ({os.path.getsize(weight_path)} bytes)")

    # -- 2. Load via torch.hub (clones ultralytics/yolov5 v7.0) ------------
    # Pin to v7.0 — the last standalone release before yolov5 master
    # started importing from the `ultralytics` package.
    #
    # PyTorch ≥2.6 defaults torch.load to weights_only=True, but the
    # MegaDetector checkpoint contains pickled model classes
    # (models.yolo.Model) that require weights_only=False.  The yolov5
    # v7.0 hub code doesn't pass that flag, so we monkey-patch.
    _original_torch_load = torch.load
    torch.load = lambda *a, **kw: _original_torch_load(
        *a, **{**kw, "weights_only": False}
    )

    print("Loading MegaDetector v5a via torch.hub (yolov5 v7.0)...")
    hub_model = torch.hub.load(
        "ultralytics/yolov5:v7.0", "custom", path=weight_path,
        force_reload=True, trust_repo=True,
    )

    # Restore original torch.load
    torch.load = _original_torch_load
    det_model = hub_model.model          # inner DetectionModel
    det_model.float().eval()

    # Put the Detect head into export mode so it concatenates raw outputs
    # into a single [batch, N, 5+num_classes] tensor.
    for m in det_model.modules():
        cls_name = type(m).__name__
        if cls_name == "Detect":
            m.export = True
            m.inplace = False

    # -- 3. Export to ONNX --------------------------------------------------
    # Fixed batch=1 — tract-onnx cannot unify a symbolic "batch" dimension
    # with literal 1 values that YOLOv5's internal reshapes produce.
    # We never run with batch>1 so static shape is fine.
    print("Exporting MegaDetector to ONNX...")
    dummy = torch.zeros(1, 3, 640, 640)
    torch.onnx.export(
        det_model, dummy, onnx_path,
        opset_version=18,
        input_names=["images"],
        output_names=["output"],
    )

    # -- 4. Internalize external data ---------------------------------------
    # PyTorch ≥2.6 may write a separate .onnx.data file for large models.
    # tract-onnx expects a single self-contained .onnx, so we reload with
    # external data and re-save as one file.
    data_file = onnx_path + ".data"
    if os.path.exists(data_file):
        print("  Internalizing external tensor data into single .onnx file...")
        model_onnx = onnx.load(onnx_path, load_external_data=True)
        onnx.save_model(
            model_onnx, onnx_path,
            save_as_external_data=False,
        )
        os.remove(data_file)
        print(f"  Single-file ONNX: {os.path.getsize(onnx_path):,} bytes")

    # -- 5. Simplify (constant-fold for tract compatibility) ----------------
    print("Simplifying ONNX graph...")
    model_onnx = onnx.load(onnx_path)
    simplified, ok = onnx_simplify(model_onnx)
    if ok:
        onnx.save(simplified, onnx_path)
        print("  graph simplified successfully")
    else:
        print("  WARNING: onnxsim could not simplify, using raw graph")

    # -- 6. Validate with onnxruntime ---------------------------------------
    sess = ort.InferenceSession(onnx_path)
    result = sess.run(None, {"images": np.zeros((1, 3, 640, 640), dtype=np.float32)})
    shape = result[0].shape
    print(f"  ONNX validation OK — output shape: {shape}")
    assert len(shape) == 3 and shape[2] == 8, (
        f"Expected [1,N,8] got {shape}; model has {shape[2]-5} classes "
        f"(expected 3: animal/person/vehicle)"
    )

    print(f"MegaDetector ONNX ready: {onnx_path} "
          f"({os.path.getsize(onnx_path):,} bytes)")

    # Clean up .pt to save image space
    os.remove(weight_path)


def _download(url: str, dest: str) -> None:
    """Download a file with ``requests`` (handles HuggingFace xet/LFS redirects).

    Falls back to ``urllib`` if ``requests`` is not available.
    """
    try:
        import requests as _req
        print(f"  GET {url}")
        resp = _req.get(url, stream=True, timeout=600, allow_redirects=True)
        resp.raise_for_status()
        with open(dest, "wb") as f:
            for chunk in resp.iter_content(chunk_size=1 << 20):
                f.write(chunk)
    except ImportError:
        urllib.request.urlretrieve(url, dest)


def export_speciesnet(output_dir: str) -> None:
    """Download SpeciesNet v4.0.1a weights from Addax and export to ONNX.

    Source: Addax-Data-Science/SPECIESNET-v4-0-1-A-v1 on HuggingFace
    (public, Apache 2.0 — original Google weights redistributed).

    Architecture: EfficientNet V2 M, 480×480 input, ~2000+ species.
    The model expects NHWC input (batch, height, width, channels).
    We wrap it with an NCHW→NHWC permute so the ONNX model accepts
    standard NCHW layout, consistent with MegaDetector.
    """
    import numpy as np
    import onnx
    import onnxruntime as ort
    import torch
    import timm  # noqa: F401 — must be importable for torch.load to unpickle

    # Addax HuggingFace repo — public, no auth needed
    base_url = (
        "https://huggingface.co/Addax-Data-Science/"
        "SPECIESNET-v4-0-1-A-v1/resolve/main"
    )
    pt_url = f"{base_url}/always_crop_99710272_22x8_v12_epoch_00148.pt"
    labels_url = f"{base_url}/always_crop_99710272_22x8_v12_epoch_00148.labels.txt"

    pt_path = os.path.join(output_dir, "speciesnet.pt")
    onnx_path = os.path.join(output_dir, "speciesnet.onnx")
    labels_dest = os.path.join(output_dir, "speciesnet_labels.txt")

    # -- 1. Download labels ------------------------------------------------
    if not os.path.exists(labels_dest):
        print(f"Downloading SpeciesNet labels → {labels_dest}")
        _download(labels_url, labels_dest)
        n_labels = sum(1 for line in open(labels_dest) if line.strip())
        print(f"Downloaded labels ({n_labels} classes)")
    else:
        n_labels = sum(1 for line in open(labels_dest) if line.strip())
        print(f"Labels already exist: {labels_dest} ({n_labels} classes)")

    # -- 2. Download PyTorch weights ---------------------------------------
    if not os.path.exists(pt_path):
        print(f"Downloading SpeciesNet v4.0.1a weights → {pt_path}")
        _download(pt_url, pt_path)
        print(f"Downloaded ({os.path.getsize(pt_path):,} bytes)")

    # -- 3. Load the model -------------------------------------------------
    print("Loading SpeciesNet PyTorch model...")
    print(f"  (timm version: {timm.__version__})")
    model = torch.load(pt_path, map_location="cpu", weights_only=False)
    print(f"  Loaded model type: {type(model).__module__}.{type(model).__name__}")
    model.eval()

    # The Google model expects NHWC input (batch, H, W, C).
    # Wrap it to accept standard NCHW so our Rust code stays consistent.
    class NCHWWrapper(torch.nn.Module):
        def __init__(self, inner):
            super().__init__()
            self.inner = inner

        def forward(self, x):
            # [B, C, H, W] → [B, H, W, C]
            return self.inner(x.permute(0, 2, 3, 1))

    wrapped = NCHWWrapper(model)
    wrapped.eval()

    # -- 4. Export to ONNX -------------------------------------------------
    # The .pt is a torch.fx.GraphModule (created by onnx2torch) with
    # data-dependent shapes.  PyTorch ≥2.6's new dynamo-based ONNX
    # exporter chokes on these with GuardOnDataDependentSymNode.
    # Force the legacy TorchScript-based exporter via dynamo=False.
    print("Exporting SpeciesNet to ONNX (NCHW input, legacy exporter)...")
    dummy = torch.zeros(1, 3, 480, 480)

    export_kwargs: dict = dict(
        opset_version=18,
        input_names=["input"],
        output_names=["logits"],
    )

    # dynamo=False was added in PyTorch 2.6 — guard for older versions.
    import inspect
    if "dynamo" in inspect.signature(torch.onnx.export).parameters:
        export_kwargs["dynamo"] = False
        print("  Using legacy TorchScript exporter (dynamo=False)")
    else:
        print("  PyTorch < 2.6 — legacy exporter is the default")

    torch.onnx.export(wrapped, dummy, onnx_path, **export_kwargs)

    # -- 5. Internalize external data if needed ----------------------------
    data_file = onnx_path + ".data"
    if os.path.exists(data_file):
        print("  Internalizing external tensor data into single .onnx file...")
        model_onnx = onnx.load(onnx_path, load_external_data=True)
        onnx.save_model(model_onnx, onnx_path, save_as_external_data=False)
        os.remove(data_file)

    # -- 6. Validate with onnxruntime --------------------------------------
    print("Validating SpeciesNet ONNX with onnxruntime...")
    sess = ort.InferenceSession(onnx_path)
    result = sess.run(None, {"input": np.zeros((1, 3, 480, 480), dtype=np.float32)})
    shape = result[0].shape
    print(f"  ONNX validation OK — output shape: {shape}")
    assert len(shape) == 2, f"Expected 2D output [1, C], got shape {shape}"
    assert shape[1] == n_labels, (
        f"Label count mismatch: ONNX outputs {shape[1]} classes "
        f"but labels file has {n_labels}"
    )

    print(f"SpeciesNet ONNX ready: {onnx_path} "
          f"({os.path.getsize(onnx_path):,} bytes, {n_labels} classes)")

    # Clean up .pt to save image space
    os.remove(pt_path)


def main() -> None:
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <output_dir>")
        sys.exit(1)

    output_dir = sys.argv[1]
    os.makedirs(output_dir, exist_ok=True)

    print("=" * 60)
    print("Gaia Light — Model Export")
    print("=" * 60)

    # MegaDetector is required
    export_megadetector(output_dir)

    # SpeciesNet is optional — classifier adds species labels but
    # the system works without it (detections still happen)
    try:
        export_speciesnet(output_dir)
    except Exception as e:
        print(f"WARNING: SpeciesNet export failed: {e}")
        traceback.print_exc()
        print("  The detector will still work without species ID.")

    print()
    print("Model export complete. Files in", output_dir + ":")
    for f in sorted(os.listdir(output_dir)):
        size = os.path.getsize(os.path.join(output_dir, f))
        print(f"  {f:30s}  {size:>12,} bytes")


if __name__ == "__main__":
    main()
