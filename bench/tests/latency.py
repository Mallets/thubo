import numpy as np
import pandas as pd
import seaborn as sns
import matplotlib.pyplot as plt
from matplotlib.ticker import EngFormatter

def expand_to_log10_bounds(ymin, ymax):
    lo = 10 ** np.floor(np.log10(ymin))
    hi = 10 ** np.ceil(np.log10(ymax))
    return lo, hi

def binary_size_label(x):
    if x <= 0:
        return "0"

    p = int(np.ceil(np.log2(x)))
    v = 2 ** p
    
    if v >= 2**40:
        return f"{v / 2**30:.0f} Ti"
    elif v >= 2**30:
        return f"{v / 2**30:.0f} Gi"
    elif v >= 2**20:
        return f"{v / 2**20:.0f} Mi"
    elif v >= 2**10:
        return f"{v / 2**10:.0f} Ki"
    else:
        return str(v)
    
def annotate_points(ax, x, y, formatter=str, offset=9, rotation=315):
    for xi, yi in zip(x, y):
        ax.annotate(
            formatter(yi),
            (xi, yi),
            textcoords="offset points",
            xytext=(-offset, offset),
            ha="center",
            rotation=rotation,
            fontsize=9
        )

csv_path = "/tmp/ping.log"

df = pd.read_csv(csv_path,
    header=None,
    names=["size", "seq", "rtt_ns"]
)

# ---- Compute RTT in seconds (input is nanoseconds) ----
df["rtt_s"] = df["rtt_ns"].astype(float) / 1000000000

# ---- Median aggregation ----
agg = (
    df.groupby("size", as_index=False)
      .agg(
          rtt_s=("rtt_s", "median"),
      )
)

# Treat size as categorical
agg["size_label"] = agg["size"].apply(binary_size_label)

# ---- Plot ----
sns.set_theme(style="whitegrid")

fig, ax = plt.subplots(figsize=(12, 5))

# ---- msg/s subplot ----
sns.lineplot(
    data=agg,
    x="size_label",
    y="rtt_s",
    marker="o",
    errorbar="ci",
    ax=ax
)

annotate_points(
    ax,
    agg["size_label"],
    agg["rtt_s"],
    formatter=lambda v: EngFormatter(places=1).format_eng(v),
)

ax.set_title("Latency over TCP (localhost) - Apple M4")

ax.set_xlabel("Message Size (bytes)")
ax.set_ylabel("RTT (s)")
ax.set_yscale("log")
ax.yaxis.set_major_formatter(EngFormatter())
ax.grid(True, which="minor", axis="y", linestyle=":", linewidth=0.7, alpha=0.4)

ymin, ymax = ax.get_ylim()
lo, hi = expand_to_log10_bounds(ymin, ymax)
ax.set_ylim(lo, hi)

plt.xticks(rotation=45)
plt.tight_layout()
plt.savefig(
    "latency.svg",
    format="svg",
    bbox_inches="tight"
)
