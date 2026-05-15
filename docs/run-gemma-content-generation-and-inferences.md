# Run Gemma content generation and inferences

There are two key decisions to make when you want to run a Gemma model:
1) what Gemma variant you want to run, and 2) what AI execution framework you
are going to use to run it? A key issue in making both these decisions has to do
with what hardware you and your users have available to run the model.

This overview helps you navigate these decisions and start working with Gemma
models. The general steps for running a Gemma model are as follows:

- [Choose a framework for running](https://ai.google.dev/gemma/docs/run#choose-a-framework)
- [Select a Gemma variant](https://ai.google.dev/gemma/docs/run#select-a-variant)
- [Run generation and inference requests](https://ai.google.dev/gemma/docs/run#run-generation)

## Choose a framework

Gemma models are compatible with a wide variety of ecosystem tools. Choosing the
right one depends on your available hardware (Cloud GPUs versus Local Laptop)
and your interface preference (Python code versus Desktop Application).

Use the following table to quickly identify the best tool for your needs:

| If you want to... | Recommended Framework | Best For |
|---|---|---|
| **Run locally with a Chat UI** | - **[LM Studio](https://ai.google.dev/gemma/docs/integrations/lmstudio)** - **[Ollama](https://ai.google.dev/gemma/docs/integrations/ollama)** | Beginners, or users who want a "Gemini-like" experience on their laptop. |
| **Run efficiently on Edge** | - **[LiteRT-LM](https://ai.google.dev/edge/litert-lm/overview)** - **[llama.cpp](https://ai.google.dev/gemma/docs/integrations/llamacpp)** - **[MediaPipe LLM Inference API](https://ai.google.dev/edge/mediapipe/solutions/genai/llm_inference)** - **[MLX](https://ai.google.dev/gemma/docs/integrations/mlx)** | High-performance local inference with minimal resources. |
| **Build/Train in Python** | - **[Gemma library for JAX](https://gemma-llm.readthedocs.io)** - **[Hugging Face Transformers](https://huggingface.co/docs/transformers/en/model_doc/gemma4)** - **[Keras](https://ai.google.dev/gemma/docs/core/keras_inference)** - **[Unsloth](https://unsloth.ai/docs/models/gemma-4)** | Researchers and Developers building custom applications or fine-tuning models. |
| **Deploy to Production / Enterprise** | - **[Google Cloud Kubernetes Engine (GKE)](https://ai.google.dev/gemma/docs/core/gke)** - **[Google Cloud Run](https://ai.google.dev/gemma/docs/core/deploy_to_cloud_run_from_ai_studio)** - **[Vertex AI](https://docs.cloud.google.com/vertex-ai/generative-ai/docs/open-models/use-gemma)** - **[vLLM](https://docs.cloud.google.com/kubernetes-engine/docs/tutorials/serve-gemma-gpu-vllm)** | Scalable, managed cloud deployment with enterprise security and MLOps support. |

### Framework Details

The following are guides for running Gemma models categorized by your deployment
environment.

#### 1. Desktop \& Local Inference (High Efficiency)

These tools allow you to run Gemma on consumer hardware (laptops, desktops) by
utilizing optimized formats (like GGUF) or specific hardware accelerators.

- **[LM Studio](https://ai.google.dev/gemma/docs/integrations/lmstudio)**: A desktop application that lets you download and chat with Gemma models in a user-friendly interface. No coding required.
- **[llama.cpp](https://ai.google.dev/gemma/docs/integrations/llamacpp)**: A popular open-source C++ port of Llama (and Gemma) that runs incredibly fast on CPUs and Apple Silicon.
- **[LiteRT-LM](https://ai.google.dev/edge/litert-lm/overview)** : Offers a command-line interface ([CLI](https://ai.google.dev/edge/litert-lm/cli)) to run optimized `.litertlm` Gemma models on desktop (Windows, Linux, macOS), powered by LiteRT (formerly TFLite).
- **[MLX](https://ai.google.dev/gemma/docs/integrations/mlx)**: A framework designed specifically for machine learning on
  Apple Silicon, perfect for Mac users who want built-in performance.

- **[Ollama](https://ai.google.dev/gemma/docs/integrations/ollama)**: A tool to run open LLMs locally, often used to power
  other applications.

#### 2. Python Development (Research \& Fine-tuning)

Standard frameworks for AI developers building applications, pipelines, or
training models.

- **[Hugging Face Transformers](https://huggingface.co/docs/transformers/en/model_doc/gemma4)**: The industry standard for quick access to models and pipelines.
- **[Unsloth](https://unsloth.ai/docs/models/gemma-4)**: An optimized library for fine-tuning LLMs. It lets you train Gemma models 2-5x faster with significantly less memory, making it possible to fine-tune on consumer GPUs (e.g., free Google Colab tiers).
- **[Keras](https://ai.google.dev/gemma/docs/core/keras_inference)** / **[JAX](https://gemma-llm.readthedocs.io)**: Core libraries for deep learning research and custom architecture implementation.

#### 3. Mobile \& Edge Deployment (On-Device)

Frameworks designed to run LLMs directly on user devices (Android, iOS, Web)
without internet connectivity, often utilizing NPUs (Neural Processing Units).

- **[LiteRT-LM](https://ai.google.dev/edge/litert-lm/overview)**: The fully open-source framework for on-device LLM development that offers maximum performance and fine-grained control, with direct support for CPU, GPU, and NPU acceleration on Android and iOS.
- **[MediaPipe LLM Inference API](https://ai.google.dev/edge/mediapipe/solutions/genai/llm_inference)**: The easiest way to integrate Gemma into cross-platform apps. It offers a high-level API that works across Android, iOS, and Web.

#### 4. Cloud \& Production Deployment

Managed services for scaling your application to thousands of users or accessing
massive compute power.

- **[Vertex AI](https://docs.cloud.google.com/vertex-ai/generative-ai/docs/open-models/use-gemma)**: Google Cloud's fully managed AI platform. Best for enterprise applications requiring SLAs and scaling.
- **[Google Cloud Kubernetes Engine (GKE)](https://ai.google.dev/gemma/docs/core/gke)**: For orchestrating your own serving clusters.
- **[vLLM](https://docs.cloud.google.com/kubernetes-engine/docs/tutorials/serve-gemma-gpu-vllm)**: A high-throughput and memory-efficient inference and serving engine, often used in cloud deployments.

Make sure your intended deployment Gemma model format, such as Keras built-in
format, Safetensors, or GGUF, is supported by your chosen framework.

## Select a Gemma variant

Gemma models are available in several variants and sizes, including the
foundation or [core](https://ai.google.dev/gemma/docs/core) Gemma models, and more
specialized model variants such as
[PaliGemma](https://ai.google.dev/gemma/docs/paligemma) and
[DataGemma](https://ai.google.dev/gemma/docs/datagemma), and many variants
created by the AI developer community on sites such as
[Kaggle](https://www.kaggle.com/models?query=gemma) and
[Hugging Face](https://huggingface.co/models?search=gemma). If you are unsure
about what variant you should start with, select the latest Gemma
[core](https://ai.google.dev/gemma/docs/core) instruction-tuned (IT) model with
the lowest number of parameters. This type of Gemma model has low compute
requirements and be able to respond to a wide variety of prompts without
requiring additional development.

Consider the following factors when choosing a Gemma variant:

- **Gemma core, and other variant families such as PaliGemma, CodeGemma** : *Recommend Gemma (core).* Gemma variants beyond the core version have the same architecture as the core model, and are trained to perform better at specific tasks. Unless your application or goals align with the specialization of a specific Gemma variant, it is best to start with a Gemma core, or base, model.
- **Instruction-tuned (IT), pre-trained (PT), fine-tuned (FT), mixed
  (mix)** : *Recommend IT.*
  - *Instruction-tuned* (IT) Gemma variants are models that have been trained to respond to a variety of instructions or requests in human language. These model variants are the best place to start because they can respond to prompts without further model training.
  - *Pre-trained* (PT) Gemma variants are models that have been trained to make inferences about language or other data, but have not been trained to follow human instructions. These models require additional training or tuning to be able to perform tasks effectively, and are meant for researchers or developers who want to study or develop the capabilities of the model and its architecture.
  - *Fine-tuned* (FT) Gemma variants can be considered IT variants, but are typically trained to perform a specific task, or perform well on a specific generative AI benchmark. The PaliGemma variant family includes a number of FT variants.
  - *Mixed* (mix) Gemma variants are versions of PaliGemma models that have been instruction tuned with a variety of instructions and are suitable for general use.
- **Parameters** : *Recommend smallest number available*. In general, the more parameters a model has, the more capable it is. However, running larger models requires larger and more complex compute resources, and generally slows down development of an AI application. Unless you have already determined that a smaller Gemma model cannot meet your needs, choose a one with a small number of parameters.
- **Quantization levels:** *Recommend half precision (16-bit), except for
  tuning*. Quantization is a complex topic that boils down to what size and precision of data, and consequently how much memory a generative AI model uses for calculations and generating responses. After a model is trained with high-precision data, which is typically 32-bit floating point data, models like Gemma can be modified to use lower precision data such as 16, 8 or 4-bit sizes. These quantized Gemma models can still perform well, depending on the complexity of the tasks, while using significantly less compute and memory resources. However, tools for tuning quantized models are limited and may not be available within your chosen AI development framework. Typically, you must fine-tune a model like Gemma at full precision, then quantize the resulting model.

For a list of key, Google-published Gemma models, see the
[Getting started with Gemma models](https://ai.google.dev/gemma/docs/get_started#models-list),
Gemma model list.

## Run generation and inference requests

After you have selected an AI execution framework and a Gemma variant, you can
start running the model, and prompting it to generate content or complete tasks.
For more information on how to run Gemma with a specific framework, see the
guides linked in the [Choose a framework](https://ai.google.dev/gemma/docs/run#choose-a-framework) section.

### Prompt formatting

All instruction-tuned Gemma variants have specific prompt formatting
requirements. Some of these formatting requirements are handled automatically by
the framework you use to run Gemma models, but when you are sending prompt data
directly to a tokenizer, you must add specific tags, and the tagging
requirements can change depending on the Gemma variant you are using. See the
following guides for information on Gemma variant prompt formatting and system
instructions:

- [Gemma prompt and system instructions](https://ai.google.dev/gemma/docs/core/prompt-formatting-gemma4)
- [PaliGemma prompt and system instructions](https://ai.google.dev/gemma/docs/paligemma/prompt-system-instructions)
- [FunctionGemma formatting and best practices](https://ai.google.dev/gemma/docs/functiongemma/formatting-and-best-practices)