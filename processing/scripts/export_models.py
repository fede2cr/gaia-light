#!/usr/bin/env python3
"""Export MegaDetector v5a to ONNX and download SpeciesNet files.

Called during container image build (see Containerfile, converter stage).

Usage:
    python export_models.py /output/dir

Produces:
    /output/dir/megadetector_v6.onnx      (YOLOv5-based detector)
    /output/dir/speciesnet.onnx            (species classifier)
    /output/dir/speciesnet_labels.txt      (species label list)
"""

import os
import sys
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


def download_speciesnet(output_dir: str) -> None:
    """Download SpeciesNet ONNX model and labels from HuggingFace.

    Requires a HuggingFace token for gated repos.  The token is read
    from (in order):
      1. The file ``/run/secrets/hf_token`` (Podman/Docker build secret)
      2. The ``HF_TOKEN`` environment variable

    If no token is available the download will likely fail with HTTP 401
    for gated models.  This is non-fatal — the detector still works.
    """
    files = {
        "speciesnet.onnx": (
            "https://huggingface.co/google/speciesnet/resolve/main/"
            "speciesnet.onnx"
        ),
        "speciesnet_labels.txt": (
            "https://huggingface.co/google/speciesnet/resolve/main/"
            "speciesnet_labels.txt"
        ),
    }

    # Resolve HuggingFace token
    hf_token = _read_hf_token()
    if hf_token:
        print("HuggingFace token found — will use for authenticated downloads")
    else:
        print("WARNING: No HF_TOKEN found. Gated model downloads may fail (HTTP 401).")

    for filename, url in files.items():
        dest = os.path.join(output_dir, filename)
        if os.path.exists(dest):
            print(f"Already exists: {dest}")
            continue

        print(f"Downloading {filename} from {url}")
        try:
            req = urllib.request.Request(url)
            if hf_token:
                req.add_header("Authorization", f"Bearer {hf_token}")
            with urllib.request.urlopen(req) as resp:
                data = resp.read()
            with open(dest, "wb") as f:
                f.write(data)
            print(f"Downloaded {filename} ({os.path.getsize(dest)} bytes)")
        except Exception as e:
            print(f"WARNING: Cannot download {filename}: {e}")
            print("  SpeciesNet classifier will not be available.")
            print("  The detector will still work without species ID.")


def _read_hf_token() -> str | None:
    """Read the HuggingFace token from a build secret or env var."""
    # 1. Podman/Docker build secret (preferred — never leaks into layers)
    secret_path = "/run/secrets/hf_token"
    if os.path.isfile(secret_path):
        token = open(secret_path).read().strip()
        if token:
            return token
    # 2. Environment variable fallback
    token = os.environ.get("HF_TOKEN", "").strip()
    return token or None


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
    download_speciesnet(output_dir)

    print()
    print("Model export complete. Files in", output_dir + ":")
    for f in sorted(os.listdir(output_dir)):
        size = os.path.getsize(os.path.join(output_dir, f))
        print(f"  {f:30s}  {size:>12,} bytes")


if __name__ == "__main__":
    main()
