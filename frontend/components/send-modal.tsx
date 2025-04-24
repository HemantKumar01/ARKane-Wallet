"use client";

import { useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { toast } from "sonner";
import { Loader2 } from "lucide-react";

interface SendModalProps {
  isOpen: boolean;
  onClose: () => void;
  walletId: string;
  onSuccess: () => void;
}

export function SendModal({
  isOpen,
  onClose,
  walletId,
  onSuccess,
}: SendModalProps) {
  const [address, setAddress] = useState("");
  const [amount, setAmount] = useState("");
  const [paymentType, setPaymentType] = useState("offchain");
  const [isLoading, setIsLoading] = useState(false);

  const handleSend = async () => {
    if (!address.trim() || !amount.trim()) {
      toast.error("Error", {
        description: "Please enter both address and amount",
      });
      return;
    }

    setIsLoading(true);

    try {
      if (paymentType === "offchain") {
        // Send to ARK address (offchain)
        console.log(
          JSON.stringify({
            wallet_id: walletId,
            address: address,
            amount: Number.parseFloat(amount),
          })
        );

        const response = await fetch(
          "http://localhost:8080/send_to_ark_address",
          {
            method: "POST",
            headers: {
              "Content-Type": "application/json",
            },

            body: JSON.stringify({
              wallet_id: walletId,
              address: address,
              amount: Number.parseFloat(amount),
            }),
          }
        );
        console.log(response);
        const data = await response.json();
        if (!data.txid) {
          throw new Error(data.error || "Payment failed");
        }

        toast.success("Payment Sent", {
          description: `${amount} BTC sent to ${address.substring(0, 8)}...`,
        });
      } else {
        // Onchain payment - placeholder for now
        // In a real implementation, you would call your onchain payment API
        await new Promise((resolve) => setTimeout(resolve, 1500));

        toast.success("Onchain Payment Sent", {
          description: `${amount} BTC sent to ${address.substring(0, 8)}...`,
        });
      }

      onSuccess();
      onClose();
      setAddress("");
      setAmount("");
    } catch (error) {
      console.error("Payment error:", error);
      toast.error("Error", {
        description:
          error instanceof Error ? error.message : "Failed to send payment",
      });
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={onClose}>
      <DialogContent className="sm:max-w-md border-zinc-800 bg-zinc-950">
        <DialogHeader>
          <DialogTitle>Send Bitcoin</DialogTitle>
          <DialogDescription>
            Send Bitcoin to another wallet address
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-4 py-4">
          <div className="grid gap-2">
            <Label htmlFor="payment-type">Payment Type</Label>
            <RadioGroup
              id="payment-type"
              value={paymentType}
              onValueChange={setPaymentType}
              className="flex space-x-4"
            >
              <div className="flex items-center space-x-2">
                <RadioGroupItem value="offchain" id="offchain" />
                <Label htmlFor="offchain">Offchain</Label>
              </div>
              <div className="flex items-center space-x-2">
                <RadioGroupItem value="onchain" id="onchain" />
                <Label htmlFor="onchain">Onchain</Label>
              </div>
            </RadioGroup>
          </div>
          <div className="grid gap-2">
            <Label htmlFor="address">Recipient Address</Label>
            <Input
              id="address"
              value={address}
              onChange={(e) => setAddress(e.target.value)}
              placeholder="Enter recipient address"
              className="bg-zinc-900 border-zinc-800"
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="amount">Amount (BTC)</Label>
            <Input
              id="amount"
              value={amount}
              onChange={(e) => setAmount(e.target.value)}
              placeholder="0.00000000"
              type="number"
              step="0.00000001"
              min="0"
              className="bg-zinc-900 border-zinc-800"
            />
          </div>
        </div>
        <DialogFooter>
          <Button
            variant="outline"
            onClick={onClose}
            className="border-zinc-800"
          >
            Cancel
          </Button>
          <Button
            onClick={handleSend}
            disabled={isLoading}
            className="bg-orange-600 hover:bg-orange-700"
          >
            {isLoading ? (
              <>
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                Sending...
              </>
            ) : (
              "Send"
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
