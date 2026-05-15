# Thinking mode in Gemma

|---|---|---|---|---|
| [![](https://ai.google.dev/static/site-assets/images/docs/notebook-site-button.png)View on ai.google.dev](https://ai.google.dev/gemma/docs/capabilities/thinking) | [![](https://www.tensorflow.org/images/colab_logo_32px.png)Run in Google Colab](https://colab.research.google.com/github/google-gemma/cookbook/blob/main/docs/capabilities/thinking.ipynb) | [![](https://www.kaggle.com/static/images/logos/kaggle-logo-transparent-300.png)Run in Kaggle](https://kaggle.com/kernels/welcome?src=https://github.com/google-gemma/cookbook/blob/main/docs/capabilities/thinking.ipynb) | [![](https://ai.google.dev/images/cloud-icon.svg)Open in Vertex AI](https://console.cloud.google.com/vertex-ai/colab/import/https%3A%2F%2Fraw.githubusercontent.com%2Fgoogle-gemma%2Fcookbook%2Fmain%2Fdocs%2Fcapabilities%2Fthinking.ipynb) | [![](https://www.tensorflow.org/images/GitHub-Mark-32px.png)View source on GitHub](https://github.com/google-gemma/cookbook/blob/main/docs/capabilities/thinking.ipynb) |

Gemma is a family of lightweight, state-of-the-art open models built from the same research and technology used to create the [Gemini](https://deepmind.google/technologies/gemini/#introduction) models. Gemma 4 is designed to be the world's most efficient open-weight model family.

This document demonstrates how to use the thinking capabilities of Gemma 4 to generate reasoning processes before providing a final answer. You will learn how to enable thinking mode for both text-only and multimodal (image-text) tasks using the Hugging Face `transformers` library, and how to parse the output to separate thinking from the answer.

This notebook will run on T4 GPU.

## Install Python packages

Install the Hugging Face libraries required for running the Gemma model and making requests.

    # Install PyTorch & other libraries
    pip install torch accelerate

    # Install the transformers library
    pip install -U transformers

## Load Model

Use the `transformers` libraries to create an instance of a `processor` and `model` using the `AutoProcessor` and `AutoModelForImageTextToText` classes as shown in the following code example:

    MODEL_ID = "google/gemma-4-E2B-it" # @param ["google/gemma-4-E2B-it","google/gemma-4-E4B-it", "google/gemma-4-31B-it", "google/gemma-4-26B-A4B-it"]

    from transformers import AutoProcessor, AutoModelForMultimodalLM

    model = AutoModelForMultimodalLM.from_pretrained(MODEL_ID, dtype="auto", device_map="auto")
    processor = AutoProcessor.from_pretrained(MODEL_ID)

```
model.safetensors:   0%|          | 0.00/10.2G [00:00<?, ?B/s]
Loading weights:   0%|          | 0/1951 [00:00<?, ?it/s]
generation_config.json:   0%|          | 0.00/208 [00:00<?, ?B/s]
processor_config.json: 0.00B [00:00, ?B/s]
chat_template.jinja: 0.00B [00:00, ?B/s]
tokenizer_config.json: 0.00B [00:00, ?B/s]
tokenizer.json:   0%|          | 0.00/32.2M [00:00<?, ?B/s]
```

## A single text inference with Thinking

To generate a response using the model's thinking capabilities, pass `enable_thinking=True`, the processor will insert the correct thinking tokens into the prompt, instructing the model to think before responding.
>
> > [!NOTE]
> > **Note:** We've added an empty thinking token to the chat template for `gemma-4-26B-A4B-it` and `gemma-4-31B-it`. This stabilizes model output by suppressing "ghost" thought channels that may appear even when thinking is deactivated.
>
| Model Size | Thinking State | Template Structure / Output |
|---|---|---|
| **E2B/E4B** | **OFF** | `<|turn>user\n[Prompt]<turn|>\n<|turn>model` |
| **E2B/E4B** | **ON** | `<|turn>system\n<|think|><turn|>\n<|turn>user\n[Prompt]<turn|>\n<|turn>model` |
| **26B/31B** | **OFF** | ⚠️ `<|turn>user\n[Prompt]<turn|>\n<|turn>model\n<|channel>thought\n<channel|>` |
| **26B/31B** | **ON** | `<|turn>system\n<|think|><turn|>\n<|turn>user\n[Prompt]<turn|>\n<|turn>model` |

    from transformers import TextStreamer

    message = [
        {
            "role": "user", "content": "What is the water formula?"
        }
    ]

    text = processor.apply_chat_template(message, tokenize=False, add_generation_prompt=True, enable_thinking=True)
    inputs = processor(text=text, return_tensors="pt").to(model.device)
    input_len = inputs["input_ids"].shape[-1]

    streamer = TextStreamer(processor)
    outputs = model.generate(**inputs, streamer=streamer, max_new_tokens=1024)

```
<bos><|turn>system
<|think|>
<turn|>
<|turn>user
What is the water formula?<turn|>
<|turn>model
<|channel>thought
Thinking Process:

1.  **Analyze the Request:** The user is asking "What is the water formula?"

2.  **Interpret Ambiguity:** The term "water formula" is highly ambiguous. It could mean several things:
    *   **Chemical Formula:** The molecular formula of water (\\(\text{H}_2\text{O}\\)).
    *   **Hydration Formula/Concept:** How water interacts with solutes or biological systems.
    *   **Water Cycle Formula:** A diagram or mathematical representation of evaporation, condensation, precipitation.
    *   **A Specific Context (e.g., cooking, chemistry lab, a specific theory):** The user might be referring to something specific that I don't have context for.

3.  **Determine the Most Likely Interpretation (The Default Answer):** In a general knowledge context, when someone asks for a "formula" related to water, they usually mean the chemical structure.

4.  **Formulate the Chemical Answer:**
    *   Chemical Name: Water
    *   Chemical Formula: \\(\text{H}_2\text{O}\\)
    *   Structure: Two hydrogen atoms bonded to one oxygen atom.

5.  **Address Other Possible Interpretations (To ensure a complete answer):** Since the term is vague, I should acknowledge the different contexts.

6.  **Draft the Response:**
    *   Start with the most direct, scientific answer (the chemical formula).
    *   Explain what the formula means.
    *   Briefly mention other related concepts (like the water cycle) if necessary, but focus on the primary meaning.
    *   Ask for clarification if the user meant something else.

7.  **Review against Identity Constraints:** (I am Gemma 4, LLM, open weights, no tools needed). The answer is factual and does not violate any constraints.

8.  **Final Output Generation.** (Self-Correction during drafting: Ensure the chemical notation is clear.)<channel|>The term "water formula" can refer to a few different things, depending on the context. Here are the most common interpretations:

### 1. Chemical Formula (The most likely answer)

If you are referring to the **chemical structure** of water (\\(\text{H}_2\text{O}\\)), the formula is:


$$\text{H}_2\text{O}$$


**What this means:**

*   **H** stands for Hydrogen.
*   **O** stands for Oxygen.
*   The formula indicates that one molecule of water is composed of **two hydrogen atoms** chemically bonded to **one oxygen atom**.

***

### 2. The Water Cycle Formula (Scientific Process)

If you are referring to the **water cycle**, which describes the continuous movement of water on, above, and below the surface of the Earth, there isn't a single mathematical formula, but rather a series of physical processes:

*   **Evaporation:** Liquid water turns into water vapor (gas) due to heat.
*   **Condensation:** Water vapor cools and turns back into liquid water, forming clouds.
*   **Precipitation:** Water falls back to Earth in the form of rain, snow, sleet, or hail.
*   **Collection/Runoff:** Water gathers in rivers, lakes, oceans, or soaks into the ground.

***

### 3. Hydration Formula (Biology/Chemistry)

In a biological or chemical context, the "formula" might refer to how water interacts with other substances, such as:

*   **Solubility:** The ability of water to dissolve other substances (often described by "like dissolves like").
*   **Hydration Shells:** The arrangement of water molecules surrounding an ion or molecule.

**Could you please provide a little more context?** Knowing whether you are asking about chemistry, biology, or a specific concept will help me give you the exact formula you are looking for!<turn|>
```

Once the text is generated, the response will contain both the reasoning blocks and the final answer bounded by special tokens. You can use the `parse_response` utility to easily extract them into a dictionary containing `thinking` and `answer`.
>
> > [!NOTE]
> > **Note:** `skip_special_tokens=False` is mandatory to make the reasoning parser to work correctly.
>
    response = processor.decode(outputs[0][input_len:], skip_special_tokens=False)
    result = processor.parse_response(response)

    for key, value in result.items():
      if key == "role":
        print(f"Role: {value}")
      elif key == "thinking":
        print(f"\n=== Thoughts ===\n{value}")
      elif key == "content":
        print(f"\n=== Answer ===\n{value}")
      elif key == "tool_calls":
        print(f"\n=== Tool Calls ===\n{value}")
      else:
        print(f"\n{key}: {value}...\n")

```
Role: assistant

=== Thoughts ===
Thinking Process:

1.  **Analyze the Request:** The user is asking "What is the water formula?"

2.  **Interpret Ambiguity:** The term "water formula" is highly ambiguous. It could mean several things:
    *   **Chemical Formula:** The molecular formula of water (\\(\text{H}_2\text{O}\\)).
    *   **Hydration Formula/Concept:** How water interacts with solutes or biological systems.
    *   **Water Cycle Formula:** A diagram or mathematical representation of evaporation, condensation, precipitation.
    *   **A Specific Context (e.g., cooking, chemistry lab, a specific theory):** The user might be referring to something specific that I don't have context for.

3.  **Determine the Most Likely Interpretation (The Default Answer):** In a general knowledge context, when someone asks for a "formula" related to water, they usually mean the chemical structure.

4.  **Formulate the Chemical Answer:**
    *   Chemical Name: Water
    *   Chemical Formula: \\(\text{H}_2\text{O}\\)
    *   Structure: Two hydrogen atoms bonded to one oxygen atom.

5.  **Address Other Possible Interpretations (To ensure a complete answer):** Since the term is vague, I should acknowledge the different contexts.

6.  **Draft the Response:**
    *   Start with the most direct, scientific answer (the chemical formula).
    *   Explain what the formula means.
    *   Briefly mention other related concepts (like the water cycle) if necessary, but focus on the primary meaning.
    *   Ask for clarification if the user meant something else.

7.  **Review against Identity Constraints:** (I am Gemma 4, LLM, open weights, no tools needed). The answer is factual and does not violate any constraints.

8.  **Final Output Generation.** (Self-Correction during drafting: Ensure the chemical notation is clear.)

=== Answer ===
The term "water formula" can refer to a few different things, depending on the context. Here are the most common interpretations:

### 1. Chemical Formula (The most likely answer)

If you are referring to the **chemical structure** of water (\\(\text{H}_2\text{O}\\)), the formula is:


$$\text{H}_2\text{O}$$


**What this means:**

*   **H** stands for Hydrogen.
*   **O** stands for Oxygen.
*   The formula indicates that one molecule of water is composed of **two hydrogen atoms** chemically bonded to **one oxygen atom**.

***

### 2. The Water Cycle Formula (Scientific Process)

If you are referring to the **water cycle**, which describes the continuous movement of water on, above, and below the surface of the Earth, there isn't a single mathematical formula, but rather a series of physical processes:

*   **Evaporation:** Liquid water turns into water vapor (gas) due to heat.
*   **Condensation:** Water vapor cools and turns back into liquid water, forming clouds.
*   **Precipitation:** Water falls back to Earth in the form of rain, snow, sleet, or hail.
*   **Collection/Runoff:** Water gathers in rivers, lakes, oceans, or soaks into the ground.

***

### 3. Hydration Formula (Biology/Chemistry)

In a biological or chemical context, the "formula" might refer to how water interacts with other substances, such as:

*   **Solubility:** The ability of water to dissolve other substances (often described by "like dissolves like").
*   **Hydration Shells:** The arrangement of water molecules surrounding an ion or molecule.

**Could you please provide a little more context?** Knowing whether you are asking about chemistry, biology, or a specific concept will help me give you the exact formula you are looking for!
```

## Multi-Turn Example with Thought Stripping

Properly managing the model's generated thoughts is critical for maintaining performance across multi-turn conversations.

- **Standard Multi-Turn Conversations:** You must remove (strip) the model's generated thoughts from the previous turn before passing the conversation history back to the model for the next turn. If you want to disable thinking mode mid-conversation, you can remove the `<|think|>` token when you strip the previous thoughts.
- **Function Calling (Exception):** If a single model turn involves function or tool calls, thoughts must **NOT** be removed between the function calls.
- **Maintaining Conversation History:** The historical model output must only include the final response. Ensure that no generated thoughts from previous turns remain in the context window before the next user turn begins.

    from transformers import TextStreamer

    # Append the clean response to the message history
    message.append({
        "role": "assistant",
        "content": result["content"]
    })

    # ==========================================
    # TURN 2
    # ==========================================
    print("\n--- Turn 2 ---")
    # Add the next user query to the history
    message.append({
        "role": "user",
        "content": "What is its boiling point in Celsius?"
    })

    text = processor.apply_chat_template(message, tokenize=False, add_generation_prompt=True, enable_thinking=True)
    inputs = processor(text=text, return_tensors="pt").to(model.device)
    input_len = inputs["input_ids"].shape[-1]

    streamer = TextStreamer(processor)
    outputs = model.generate(**inputs, streamer=streamer, max_new_tokens=1024)

```
--- Turn 2 ---
<bos><|turn>system
<|think|>
<turn|>
<|turn>user
What is the water formula?<turn|>
<|turn>model
The term "water formula" can refer to a few different things, depending on the context. Here are the most common interpretations:

### 1. Chemical Formula (The most likely answer)

If you are referring to the **chemical structure** of water (\\(\text{H}_2\text{O}\\)), the formula is:


$$\text{H}_2\text{O}$$


**What this means:**

*   **H** stands for Hydrogen.
*   **O** stands for Oxygen.
*   The formula indicates that one molecule of water is composed of **two hydrogen atoms** chemically bonded to **one oxygen atom**.

***

### 2. The Water Cycle Formula (Scientific Process)

If you are referring to the **water cycle**, which describes the continuous movement of water on, above, and below the surface of the Earth, there isn't a single mathematical formula, but rather a series of physical processes:

*   **Evaporation:** Liquid water turns into water vapor (gas) due to heat.
*   **Condensation:** Water vapor cools and turns back into liquid water, forming clouds.
*   **Precipitation:** Water falls back to Earth in the form of rain, snow, sleet, or hail.
*   **Collection/Runoff:** Water gathers in rivers, lakes, oceans, or soaks into the ground.

***

### 3. Hydration Formula (Biology/Chemistry)

In a biological or chemical context, the "formula" might refer to how water interacts with other substances, such as:

*   **Solubility:** The ability of water to dissolve other substances (often described by "like dissolves like").
*   **Hydration Shells:** The arrangement of water molecules surrounding an ion or molecule.

**Could you please provide a little more context?** Knowing whether you are asking about chemistry, biology, or a specific concept will help me give you the exact formula you are looking for!<turn|>
<|turn>user
What is its boiling point in Celsius?<turn|>
<|turn>model
<|channel>thought
Thinking Process:

1.  **Identify the core question:** The user is asking for the boiling point of water in Celsius.
2.  **Recall/Verify the standard scientific fact:** The standard boiling point of pure water at standard atmospheric pressure is a well-known physical constant.
3.  **State the answer clearly:** The boiling point of water is \\(100^\circ\text{C}\\).
4.  **Add necessary context/caveats (Crucial step for accuracy):** Boiling point is dependent on pressure. I must mention this to provide a complete and scientifically accurate answer.
    *   What happens at different altitudes (lower pressure)? (Boiling point drops).
    *   What happens under high pressure? (Boiling point increases).
5.  **Final Review:** The answer is direct, accurate, and provides essential context. (Self-correction: Ensure the units are explicitly stated as Celsius.)<channel|>The boiling point of pure water at standard atmospheric pressure (sea level) is:

$$\mathbf{100^\circ\text{C} }$$ (Degrees Celsius)

**Important Note:**

The boiling point of water is dependent on **pressure**.

*   **Higher Pressure:** If the pressure above the water increases (e.g., in a pressure cooker), the boiling point will be **higher** than \\(100^\circ\text{C}\\).
*   **Lower Pressure:** If the pressure decreases (e.g., at high altitudes), the boiling point will be **lower** than \\(100^\circ\text{C}\\).<turn|>
```

    response = processor.decode(outputs[0][input_len:], skip_special_tokens=False)
    result = processor.parse_response(response)

    for key, value in result.items():
      if key == "role":
        print(f"Role: {value}")
      elif key == "thinking":
        print(f"\n=== Thoughts ===\n{value}")
      elif key == "content":
        print(f"\n=== Answer ===\n{value}")
      elif key == "tool_calls":
        print(f"\n=== Tool Calls ===\n{value}")
      else:
        print(f"\n{key}: {value}...\n")

```
Role: assistant

=== Thoughts ===
Thinking Process:

1.  **Identify the core question:** The user is asking for the boiling point of water in Celsius.
2.  **Recall/Verify the standard scientific fact:** The standard boiling point of pure water at standard atmospheric pressure is a well-known physical constant.
3.  **State the answer clearly:** The boiling point of water is \\(100^\circ\text{C}\\).
4.  **Add necessary context/caveats (Crucial step for accuracy):** Boiling point is dependent on pressure. I must mention this to provide a complete and scientifically accurate answer.
    *   What happens at different altitudes (lower pressure)? (Boiling point drops).
    *   What happens under high pressure? (Boiling point increases).
5.  **Final Review:** The answer is direct, accurate, and provides essential context. (Self-correction: Ensure the units are explicitly stated as Celsius.)

=== Answer ===
The boiling point of pure water at standard atmospheric pressure (sea level) is:

$$\mathbf{100^\circ\text{C} }$$ (Degrees Celsius)

**Important Note:**

The boiling point of water is dependent on **pressure**.

*   **Higher Pressure:** If the pressure above the water increases (e.g., in a pressure cooker), the boiling point will be **higher** than \\(100^\circ\text{C}\\).
*   **Lower Pressure:** If the pressure decreases (e.g., at high altitudes), the boiling point will be **lower** than \\(100^\circ\text{C}\\).
```

## A single image inference

The procedure for using the thinking model with visual data is very similar. You can provide an image as part of the `messages` array. Just ensure you pass the image to the processor along with the formatted text, and the model will reason about the visual input before responding.

    from PIL import Image
    import matplotlib.pyplot as plt

    prompt = "What is shown in this image?"
    image_url = "https://raw.githubusercontent.com/google-gemma/cookbook/refs/heads/main/apps/sample-data/GoldenGate.png"

    # download image
    !wget -q {image_url} -O image.png
    image = Image.open("image.png")

    # Display all images
    print("=== Downloaded image ===")
    fig, ax = plt.subplots(1, 1, figsize=(5, 5))
    ax.imshow(image)
    ax.set_title("Image 1")
    ax.axis("off")
    plt.tight_layout()
    plt.show()

    message = [
        {
            "role": "user", "content": [
              {"type": "image"},
              {"type": "text", "text": prompt}
            ]
        }
    ]

    text = processor.apply_chat_template(message, tokenize=False, add_generation_prompt=True, enable_thinking=True)
    inputs = processor(text=text, images=image, return_tensors="pt").to(model.device)
    input_len = inputs["input_ids"].shape[-1]

    outputs = model.generate(**inputs, max_new_tokens=1024)
    response = processor.decode(outputs[0][input_len:], skip_special_tokens=False)

    result = processor.parse_response(response)

    for key, value in result.items():
      if key == "role":
        print(f"Role: {value}")
      elif key == "thinking":
        print(f"\n=== Thoughts ===\n{value}")
      elif key == "content":
        print(f"\n=== Answer ===\n{value}")
      elif key == "tool_calls":
        print(f"\n=== Tool Calls ===\n{value}")
      else:
        print(f"\n{key}: {value}...\n")

```
=== Downloaded image ===
```

![png](https://ai.google.dev/static/gemma/docs/capabilities/thinking_files/output_0f334997_1.png)

```
Role: assistant

=== Thoughts ===
Here's a thinking process to arrive at the suggested answer:

1.  **Analyze the Image:**
    *   **Dominant Structure:** The most prominent feature is a massive suspension bridge with tall red towers (pylons). This is immediately recognizable as the Golden Gate Bridge.
    *   **Foreground/Midground:** There is water (looks like the San Francisco Bay/Pacific Ocean). There is a breakwater/rocky shoreline in the immediate foreground. A small rock formation is visible in the water.
    *   **Background/Landmass:** Hills/mountains are visible in the distance, behind the bridge structure.
    *   **Other Elements:** There is a low-lying building/structure on the left (part of the bridge infrastructure or a related facility). The sky is clear and blue.

2.  **Identify Key Subject:** The central subject is the Golden Gate Bridge.

3.  **Determine Setting/Location (Contextualization):** Since it's the Golden Gate Bridge, the location is San Francisco, California.

4.  **Synthesize the Description (Drafting the Answer):** Start with the main subject and then add details about the setting.

    *   *Initial thought:* It's a picture of the Golden Gate Bridge over the water.
    *   *Refinement (Adding detail):* It shows the iconic red suspension bridge spanning a body of water, with land/hills in the background.

5.  **Review the Provided Text (Self-Correction/Verification):** The prompt includes some garbled text ("ołat" at the end), but the visual evidence is overwhelming. The task is simply to describe the image.

6.  **Final Output Generation:** (This leads to the structured, informative response.) (The final response should clearly identify the landmark.)

=== Answer ===
The image shows the **Golden Gate Bridge** in San Francisco, California.

Key elements visible in the picture are:

* **The Golden Gate Bridge:** The iconic red suspension bridge dominates the frame, spanning a large body of water.
* **Water:** The foreground features dark blue water, likely the Pacific Ocean or the San Francisco Bay.
* **Landmass/Hills:** Hills and mountains are visible in the background behind the bridge.
* **Foreground Detail:** There is a rocky shoreline and a small rock formation in the water in the immediate foreground.

In summary, it is a scenic view of the famous Golden Gate Bridge.
```

## Summary and next steps

In this guide, you learned how to use the thinking capabilities of Gemma 4 models to generate reasoning processes before final answers. You covered:

- Enabling thinking mode using `enable_thinking=True` in `apply_chat_template`.
- Using `TextStreamer` to observe the thinking process in real-time.
- Parsing the combined output into separate `thinking` and `answer` blocks using `parse_response`.
- Applying thinking capabilities to multimodal tasks (image + text).

### Next Steps

Explore more capabilities of Gemma 4:

- [Prompt and system instructions](https://ai.google.dev/gemma/docs/core/prompt-formatting-gemma4)
- [Function calling](https://ai.google.dev/gemma/docs/capabilities/text/function-calling-gemma4)
- [Run Gemma overview](https://ai.google.dev/gemma/docs/run)