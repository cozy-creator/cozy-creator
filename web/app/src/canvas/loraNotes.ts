/** Brief model-card summaries. Sources are retained for audit; no tag lists are presented as descriptions. */
export interface LoraNote { description: string; source: string }

const NOTES: Record<string, LoraNote> = {
  "ynamely/nsfw-posture-v3": {"description": "Adult pose control for sexual-position imagery.", "source": "https://civitai.com/models/2988215?modelVersionId=3388118"},
  "alcaitiff/unchained": {"description": "Broader anatomical concepts and prompt responses for generation and image edits.", "source": "https://civitai.com/models/3026493?modelVersionId=3437320"},
  "perpetual3x/anatomy-fix": {"description": "Improves adult genital anatomy in image generation and editing.", "source": "https://huggingface.co/perpetual3x/Qwen-Image-Genitalia-Anatomy-Fix-NSFW-LoRA-v1/raw/e5ae1a51766dc868a1b085694064e925ee22186a/README.md"},
  "thesealpacas/nsfw": {"description": "Adult anatomy, prompt-following and realism improvements for NSFW images.", "source": "https://civitai.com/models/2958918?modelVersionId=3357315"},
  "danrisi/canon": {"description": "Canon-style DSLR photography with candid exposure and soft background blur.", "source": "https://huggingface.co/Danrisi/canon_qwen2.1/raw/deaba7d4b338db928a6d11199aaa936125eff969/README.md"},
  "danrisi/samsung": {"description": "Samsung-style smartphone photos with available-light exposure and motion blur.", "source": "https://huggingface.co/Danrisi/samsung_qwen2.1/raw/299e8dbeca35f28fd41e14cdfa5e59fc24e50ae9/README.md"},
  "e-n-v-y/fix": {"description": "Targets common Qwen Image 2.1 generation issues using its supplied workflow settings.", "source": "https://huggingface.co/e-n-v-y/Qwen-Image-2.1-Fix/raw/main/README.md"},
  "warmbloodaban/anime-consistency": {"description": "Helps keep anime characters consistent across edits and changes of viewpoint.", "source": "https://huggingface.co/WarmBloodAban/Qwen-Image-2.1-LoRAs/raw/c4ab5473bfdf585fc19cfd1e280f79d2b0c79947/README.md"},
  "akhaliq/multiple-angles": {"description": "Reshoots a subject from controlled camera azimuths and elevations.", "source": "https://huggingface.co/akhaliq/Qwen-Image-2.1-Multiple-Angles-LoRA/raw/175b29b6eb41307473eebc363fca8fc198216b60/README.md"},
  "lilylilith/anyangle": {"description": "Transfers a camera angle from a rough 3D reference while retaining the original style.", "source": "https://huggingface.co/lilylilith/QI_2.1_AnyAngle/raw/5c8b4c8e05ba6328d54bf8d34ec724fef5a414ae/README.md"},
  "ausboss/outpaint": {"description": "Extends a scene beyond its borders while keeping the source picture in place.", "source": "https://huggingface.co/ausboss/Qwen-Image-2.1-Outpaint-LoRA/raw/dca5bdc7eb328f6021f330140f64b00a820ae5cd/README.md"},
  "ausboss/outfit-swap": {"description": "Outfit swaps with less drift in the face, pose and surrounding background.", "source": "https://huggingface.co/ausboss/Qwen-Image-2.1-Outfit-Swap-Consistency-LoRA/raw/f0e0fc4e4c65527963d875536aa4e77fe1e9b165/README.md"},
  "ausboss/consistency": {"description": "Keeps unchanged areas aligned during edits; it can resist large pose or position changes.", "source": "https://huggingface.co/ausboss/Qwen-Image-2.1-Consistency-LoRA/raw/58897a02f4f60f88376eab5676738dd45e5ed1a2/README.md"},
  "ausboss/restore": {"description": "Softens harsh upscales and restores detail while staying close to the source picture.", "source": "https://huggingface.co/ausboss/Qwen-Image-2.1-Restore-LoRA/raw/a223f1255f4738b89b36d79c44d61740d3b3546f/README.md"},
  "warmbloodaban/anything2real": {"description": "Turns illustrated or stylized characters into realistic human portraits.", "source": "https://huggingface.co/WarmBloodAban/Qwen-Image-2.1-LoRAs/raw/c4ab5473bfdf585fc19cfd1e280f79d2b0c79947/README.md"},
  "ai_art_vision/breasts-slider-lora": {"description": "Adjusts breast size in either direction using signed LoRA strength.", "source": "https://civitai.com/api/v1/models/764250"},
  "iamddtla/breasts-slider-lora": {"description": "Breast-size slider with large/small prompt triggers and signed strengths.", "source": "https://civitai.com/api/v1/models/2954355"},
  "herpderpmerp/vagina-lora": {"description": "Improves the rendering of female genital anatomy.", "source": "https://civitai.com/api/v1/models/2960871"},
  "tawslora/femaledvaginus-nsfw-lora": {"description": "General adult-anatomy enhancement across different skin tones and poses.", "source": "https://civitai.com/api/v1/models/2986844"},
  "perpetual3x/genitalia-anatomy-fix-nsfw-lora": {"description": "Improves adult genital anatomy in image generation and editing.", "source": "https://huggingface.co/perpetual3x/Qwen-Image-Genitalia-Anatomy-Fix-NSFW-LoRA-v1/raw/main/README.md"},
  "ynamely/vagina-lora": {"description": "Improves female genital visibility and detail.", "source": "https://civitai.com/api/v1/models/2976277"},
  "chatrlabs/uncensored-omni-lora": {"description": "One uncensored adapter for text-to-image generation and natural-language image edits.", "source": "https://huggingface.co/chatrlabs/Qwen-Image-2.1-Uncensored-Omni-LoRA-v0.1/raw/main/README.md"},
  "nrdx/bfs-body-swap-selective-lora": {"description": "Replaces a marked person using a reference character while leaving the surrounding scene unchanged.", "source": "https://civitai.com/api/v1/models/2027766"},
  "nrdx/bfs-body-swap-lora": {"description": "Transfers a reference body and outfit while following the base image pose.", "source": "https://civitai.com/api/v1/models/2027766"},
  "nrdx/bfs-head-swap-lora": {"description": "Transfers a reference head and hair while following the base image orientation.", "source": "https://civitai.com/api/v1/models/2027766"},
  "blo01/face-glass-press-lora": {"description": "Edits a face to appear pressed against a glass surface.", "source": "https://civitai.com/api/v1/models/2986691"},
  "blo01/glass-press-lora": {"description": "Edits a character to appear pressed against glass.", "source": "https://civitai.com/api/v1/models/2985261"},
  "imb101/joi-gesture-edit-lora": {"description": "Adds adult-themed hand gestures to an existing character image.", "source": "https://civitai.com/api/v1/models/3023925"},
  "alibaba-pai/qwen-image-2.1-fun-acc": {"description": "Four-step acceleration adapter for Qwen Image 2.1 generation and image editing.", "source": "https://huggingface.co/alibaba-pai/Qwen-Image-2.1-Fun-Acc-LoRAs/raw/main/README.md"},
}

export const loraNote = (ref: string): LoraNote | undefined => NOTES[ref.split("@")[0]]
